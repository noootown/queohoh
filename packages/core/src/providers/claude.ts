import { formatEventToMarkdown } from "../event-format.js";
import type {
	BuildArgsInput,
	ParsedEvent,
	ProviderAdapter,
	UnavailableInput,
} from "./types.js";

/** Matches Claude's own "you've hit your session/usage limit" message (e.g.
 * `You've hit your session limit · resets 1pm (America/Chicago)`). Permissive
 * on the noun (session/usage) and the surrounding wording, since the exact
 * phrasing isn't a stable API contract — a false negative just falls back to
 * the generic `exit code N` reason, never a crash.
 *
 * Duplicated (not imported) from `worker.ts` for now: this is a temporary
 * overlap during the provider-adapter extraction — a later task removes the
 * originals from `worker.ts` once callers are rewired through the adapter. */
const SESSION_LIMIT_RE = /\b(?:session|usage)\s+limit\b/i;

/** Matches Anthropic's "you're out of credits/money" billing error (e.g.
 * `Your credit balance is too low to access the Anthropic API`). Distinct from
 * a session/usage limit: that resets on a timer, but this needs a top-up before
 * a rerun can succeed. Permissive on the exact wording (not a stable API
 * contract) — a false negative just falls back to the generic `exit code N`
 * reason, never a crash. Checked BEFORE `SESSION_LIMIT_RE` so the more specific
 * billing signal wins if both somehow appear.
 *
 * Duplicated (not imported) from `worker.ts` for now: see `SESSION_LIMIT_RE`. */
const OUT_OF_BUDGET_RE =
	/credit balance (?:is )?too low|insufficient credits?|out of credits?/i;

export const claudeAdapter: ProviderAdapter = {
	name: "claude",
	defaultBin: "claude",
	supportsResume: true,
	promptFileSuffix: null,
	buildArgs({
		prompt,
		model,
		resumeSessionId,
		systemPrompt,
		extraArgs,
	}: BuildArgsInput): string[] {
		return [
			"-p",
			prompt,
			"--output-format",
			"stream-json",
			"--verbose",
			"--model",
			model,
			...(resumeSessionId ? ["--resume", resumeSessionId] : []),
			...(systemPrompt ? ["--append-system-prompt", systemPrompt] : []),
			...(extraArgs ?? []),
		];
	},
	parseEvent(event: Record<string, unknown>): ParsedEvent {
		const out: ParsedEvent = {};
		if (event.session_id) out.sessionId = event.session_id as string;
		if ((event.type as string) === "result") {
			out.result = {
				text: (event.result as string) ?? "",
				costUsd:
					typeof event.total_cost_usd === "number"
						? event.total_cost_usd
						: null,
				turns: typeof event.num_turns === "number" ? event.num_turns : null,
				durationMs:
					typeof event.duration_ms === "number" ? event.duration_ms : null,
			};
		}
		const md = formatEventToMarkdown(event);
		if (md) out.transcriptMd = md;
		return out;
	},
	classifyUnavailable({ resultText }: UnavailableInput): string | null {
		// Budget first (more specific: needs a top-up), then session limit (timer reset).
		if (OUT_OF_BUDGET_RE.test(resultText)) return "out of budget";
		if (SESSION_LIMIT_RE.test(resultText)) return "session limit";
		return null;
	},
};
