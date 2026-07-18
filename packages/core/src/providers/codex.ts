import type {
	BuildArgsInput,
	ParsedEvent,
	ProviderAdapter,
	UnavailableInput,
} from "./types.js";

/** Matches auth failures (missing/expired/invalid credentials) and
 * quota/rate-limit errors from the codex CLI. Both mean the provider is
 * temporarily or structurally unusable, not that the run itself failed on
 * its merits. Checked against stderr and resultText; permissive on wording
 * since this isn't a stable API contract — a false negative just falls back
 * to the generic `exit code N` reason, never a crash. */
const UNAVAILABLE_RE =
	/\b401\b|\b429\b|unauthorized|unauthenticated|invalid api key|rate limit|quota|too many requests/i;

/** Renders a codex `assistant_message` item to markdown. codex's `exec
 * --json` event shape is unrelated to claude's stream-json shape (see
 * `formatEventToMarkdown` in `../event-format.ts`), so this is a small local
 * renderer rather than a shared helper — importing the claude formatter here
 * would also re-introduce a dependency cycle via the runner. */
function renderAssistantMessage(text: string): string {
	return text;
}

export const codexAdapter: ProviderAdapter = {
	name: "codex",
	defaultBin: "codex",
	supportsResume: true,
	promptFileSuffix: null,
	buildArgs({ prompt, model, resumeSessionId }: BuildArgsInput): string[] {
		return [
			"exec",
			...(resumeSessionId ? ["resume", resumeSessionId] : []),
			"--json",
			"--skip-git-repo-check",
			"--model",
			model,
			prompt,
		];
	},
	// TODO(codex-live): verify event schema + resume flag against a live codex
	// CLI (developers.openai.com/codex). The fixtures here (thread.started /
	// item.completed{assistant_message} / turn.completed{usage}) and the
	// `exec resume <id>` argv form are representative, not confirmed against
	// a real invocation. The CONTRACT — session id, final assistant text,
	// usage, unavailable classification — is what's load-bearing; field names
	// may need adjustment once verified live.
	parseEvent(event: Record<string, unknown>): ParsedEvent {
		const out: ParsedEvent = {};
		const type = event.type as string | undefined;

		if (type === "thread.started" && typeof event.thread_id === "string") {
			out.sessionId = event.thread_id;
		}

		if (type === "item.completed") {
			const item = event.item as Record<string, unknown> | undefined;
			if (
				item?.type === "assistant_message" &&
				typeof item.text === "string"
			) {
				out.transcriptMd = renderAssistantMessage(item.text);
			}
		}

		if (type === "turn.completed") {
			// codex's turn.completed has no turn count; 1 is a placeholder (this
			// adapter reports per-turn, not per-conversation, invocations).
			// Token usage isn't priced here, so costUsd stays null (best-effort
			// per the ParsedEvent contract); the `usage` object itself, when
			// present, still carries token counts (unpriced ≠ uncounted).
			const usage = event.usage as Record<string, unknown> | undefined;
			out.result = {
				text: "",
				costUsd: null,
				turns: 1,
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
