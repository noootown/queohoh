export interface ParsedEvent {
	sessionId?: string;
	result?: {
		text: string;
		costUsd: number | null;
		turns: number | null;
		durationMs: number | null;
	};
	transcriptMd?: string;
	/** Token-level deltas for adapters whose stream carries no full-text result
	 * event (grok): the runner accumulates these and flushes one transcript
	 * section per run. Adapters that emit `transcriptMd`/`result.text` directly
	 * (claude, codex) never set these. */
	textDelta?: string;
	thinkingDelta?: string;
}

export interface BuildArgsInput {
	prompt: string;
	model: string;
	resumeSessionId?: string;
	systemPrompt?: string;
	extraArgs?: string[];
	/** Absolute path to a temp file holding the prompt, when the adapter uses one
	 * (grok). Provided by the runner; null when the adapter takes the prompt inline. */
	promptFilePath?: string;
}

export interface UnavailableInput {
	exitCode: number;
	stderr: string;
	resultText: string;
}

export interface ProviderAdapter {
	name: string;
	defaultBin: string;
	supportsResume: boolean;
	/** Does this adapter want the prompt written to a temp file (returns a filename
	 * suffix) instead of passed inline? null = inline. Runner writes/cleans the file. */
	promptFileSuffix: string | null;
	buildArgs(input: BuildArgsInput): string[];
	parseEvent(event: Record<string, unknown>): ParsedEvent;
	/** Normalized reason ("session limit" | "out of budget" | "provider unavailable")
	 * when the failure means the provider is unavailable; null for a genuine failure. */
	classifyUnavailable(input: UnavailableInput): string | null;
}
