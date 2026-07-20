import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { getAdapter } from "../providers/index.js";
import type { Redactor } from "../redact.js";
import {
	executeRun as defaultExecuteRun,
	type ExecuteRunOptions,
	type RunResult,
} from "../runner.js";
import type { SessionLineageStore } from "../session-lineage.js";
import type { DiscussStore } from "./store.js";

export interface DiscussTurnOpts {
	store: DiscussStore;
	lineage: SessionLineageStore;
	/** Discuss-store session id (not the provider session id). */
	sessionId: string;
	turnId: string;
	/** User-facing turn body (what lands under `### User` in the session transcript). */
	prompt: string;
	systemPrompt: string;
	model: string;
	provider: string;
	cwd: string;
	timeoutMs: number;
	bin?: string;
	extraArgs?: string[];
	redact: Redactor;
	/** Injectable for tests; production uses the real `executeRun`. */
	executeRun?: (
		adapter: Parameters<typeof defaultExecuteRun>[0],
		opts: ExecuteRunOptions,
	) => Promise<RunResult>;
}

export interface DiscussTurnResult {
	exitCode: number;
	/** Provider session id produced by this turn, when any. */
	sessionId: string | null;
	error?: string;
}

/**
 * Run one read-only discuss turn against a reserved review session.
 *
 * Spawns via the provider adapter with `mode: "discuss"` (no auto-approve on
 * grok), writes turn-local events/transcript under `turns/<turnId>/`, appends
 * user+assistant sections to the session transcript, and advances
 * SessionLineageStore so the next turn resumes the tip.
 *
 * Does not mutate task queue state — DiscussStore + lineage only.
 */
export async function runDiscussTurn(
	opts: DiscussTurnOpts,
): Promise<DiscussTurnResult> {
	const meta = opts.store.get(opts.sessionId);
	if (!meta) {
		return {
			exitCode: 1,
			sessionId: null,
			error: `discuss session not found: ${opts.sessionId}`,
		};
	}

	const adapter = getAdapter(opts.provider);
	if (!adapter) {
		return {
			exitCode: 1,
			sessionId: null,
			error: `unknown provider (no adapter): ${opts.provider}`,
		};
	}

	const resume = meta.lineageRoot
		? opts.lineage.tip(meta.lineageRoot)
		: undefined;

	const turnDir = opts.store.turnDir(opts.sessionId, opts.turnId);
	const eventsPath = join(turnDir, "events.jsonl");
	const turnTranscriptPath = join(turnDir, "transcript.md");

	// Session transcript: open the turn before spawn so a mid-run crash still
	// shows the user prompt (assistant body filled after the run).
	opts.store.appendTranscript(
		opts.sessionId,
		`### User\n\n${opts.prompt}\n\n### Assistant\n\n`,
	);

	const run = opts.executeRun ?? defaultExecuteRun;
	const result = await run(adapter, {
		prompt: opts.prompt,
		model: opts.model,
		cwd: opts.cwd,
		timeoutMs: opts.timeoutMs,
		claudeBin: opts.bin,
		resumeSessionId: resume,
		systemPrompt: opts.systemPrompt,
		extraArgs: opts.extraArgs,
		eventsPath,
		transcriptPath: turnTranscriptPath,
		redact: opts.redact,
		mode: "discuss",
	});

	const turnBody = existsSync(turnTranscriptPath)
		? readFileSync(turnTranscriptPath, "utf-8")
		: "";
	opts.store.appendTranscript(opts.sessionId, `${turnBody}\n\n`);

	if (result.sessionId) {
		opts.lineage.recordProvider(result.sessionId, opts.provider);
		if (resume) {
			opts.lineage.recordFork(resume, result.sessionId);
		}
		if (!meta.lineageRoot) {
			// First successful provider session becomes the chain root; later
			// turns keep the root and advance tip via recordFork.
			opts.store.setLineageRoot(opts.sessionId, result.sessionId);
		}
	}

	return {
		exitCode: result.exitCode,
		sessionId: result.sessionId,
		...(result.exitCode !== 0 && result.stderr ? { error: result.stderr } : {}),
	};
}
