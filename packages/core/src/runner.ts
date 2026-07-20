import { type ChildProcess, spawn } from "node:child_process";
import { appendFileSync, unlinkSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { claudeAdapter, type ProviderAdapter } from "./providers/index.js";
import type { Redactor } from "./redact.js";

export { formatEventToMarkdown } from "./event-format.js";

export interface RunUsage {
	costUsd: number | null;
	turns: number | null;
	durationMs: number | null;
	/** Input/output token counts from the provider's usage reporting (see
	 * `ParsedEvent.result` in `providers/types.ts`). null when the provider
	 * emitted no result event, or its result carried no usage object. */
	inputTokens: number | null;
	outputTokens: number | null;
}

export interface RunResult {
	exitCode: number;
	timedOut: boolean;
	// The signal that terminated the child (e.g. "SIGTERM"), or null when it
	// exited normally. A stop kills the process group, so this is how a stopped
	// run is distinguished from a plain non-zero exit.
	signal: string | null;
	sessionId: string | null;
	resultText: string;
	stderr: string;
	usage: RunUsage;
}

export interface ExecuteClaudeOptions {
	prompt: string;
	model: string;
	cwd: string;
	timeoutMs: number;
	claudeBin?: string;
	resumeSessionId?: string;
	claudeArgs?: string[];
	env?: Record<string, string>;
	eventsPath: string;
	transcriptPath: string;
	redact: Redactor;
	onSpawned?: (pid: number) => void;
	/** Inactivity window: reset on every successfully-parsed stream event: if no
	 * event arrives within this window the worker is presumed wedged and killed.
	 * Defaults to [`IDLE_TIMEOUT_MS`]; injectable so tests don't wait 12 minutes.
	 * `timeoutMs` remains a separate one-shot ceiling that fires regardless of
	 * activity — the primary reaper is this idle window, `timeoutMs` is the
	 * backstop against a run that never goes silent but also never finishes. */
	idleTimeoutMs?: number;
}

export interface ExecuteRunOptions extends ExecuteClaudeOptions {
	/** Appended via `--append-system-prompt` for adapters that support it (claude). */
	systemPrompt?: string;
	/** Provider-config `args` — additional adapter-produced argv, distinct from
	 * `claudeArgs` (which stays a caller-supplied trailing passthrough for
	 * back-compat with today's claude invocation). */
	extraArgs?: string[];
	/**
	 * Forwarded to `adapter.buildArgs`. Default/`agent` keeps autonomous tool
	 * approval; `discuss` is the read-only review path (e.g. grok omits
	 * `--always-approve`). Missing means agent for back-compat.
	 */
	mode?: "agent" | "discuss";
}

/** Default inactivity window for the streaming Claude runner: 12 minutes. Reset
 * on every parsed stream event (see `handleLine` in [`executeClaude`]); a run
 * that goes silent longer than this is presumed wedged and killed, independent
 * of the overall `timeoutMs` ceiling. */
export const IDLE_TIMEOUT_MS = 12 * 60_000;

export function executeRun(
	adapter: ProviderAdapter,
	opts: ExecuteRunOptions,
): Promise<RunResult> {
	const timeoutMs = Math.max(1000, opts.timeoutMs);
	const idleTimeoutMs = Math.max(1000, opts.idleTimeoutMs ?? IDLE_TIMEOUT_MS);

	return new Promise((resolve) => {
		// Prompt-file adapters (e.g. grok) want the prompt on disk rather than
		// inline in argv; the runner owns writing/cleaning that file so adapters
		// stay pure argv/parse functions.
		const promptFilePath =
			adapter.promptFileSuffix !== null
				? join(dirname(opts.eventsPath), `prompt${adapter.promptFileSuffix}`)
				: undefined;

		// Initialize run files BEFORE spawning so a failure (e.g. missing parent
		// dir) never orphans a detached child. On failure, resolve without spawning.
		try {
			writeFileSync(opts.eventsPath, "");
			writeFileSync(opts.transcriptPath, "");
			if (promptFilePath) writeFileSync(promptFilePath, opts.prompt);
		} catch (err) {
			const msg = err instanceof Error ? err.message : String(err);
			resolve({
				exitCode: 1,
				timedOut: false,
				signal: null,
				sessionId: null,
				resultText: "",
				stderr: `Failed to initialize run files: ${msg}`,
				usage: {
					costUsd: null,
					turns: null,
					durationMs: null,
					inputTokens: null,
					outputTokens: null,
				},
			});
			return;
		}

		const args = [
			...adapter.buildArgs({
				prompt: opts.prompt,
				model: opts.model,
				resumeSessionId: opts.resumeSessionId,
				systemPrompt: opts.systemPrompt,
				extraArgs: opts.extraArgs,
				promptFilePath,
				mode: opts.mode,
			}),
			...(opts.claudeArgs ?? []),
		];

		const child: ChildProcess = spawn(
			opts.claudeBin ?? adapter.defaultBin,
			args,
			{
				env: { ...process.env, ...opts.env },
				cwd: opts.cwd,
				stdio: ["ignore", "pipe", "pipe"],
				detached: true,
			},
		);
		if (child.pid && opts.onSpawned) opts.onSpawned(child.pid);

		let stderr = "";
		let resultText = "";
		let timedOut = false;
		let sessionId: string | null = null;
		let usage: RunUsage = {
			costUsd: null,
			turns: null,
			durationMs: null,
			inputTokens: null,
			outputTokens: null,
		};
		let lineBuffer = "";
		// Token-delta accumulators for adapters (grok) whose stream carries no
		// full-text result event: deltas append here so `resultText` still has a
		// fallback at the `result` event. Stay empty for claude/codex (they never
		// set the deltas), so those paths are unaffected by any of this.
		let textAcc = "";
		let thinkingAcc = "";
		// Live transcript streaming for delta-stream adapters (grok): unlike
		// textAcc/thinkingAcc above (which exist only to feed resultText), this
		// buffer drives incremental writes to transcript.md as deltas arrive, so the
		// TUI live tail isn't empty until the run ends. `streamBuf` holds content
		// not yet flushed to disk; `streamSection` tracks which part of the old
		// batched shape ("### Thinking" header / blank-line separator / body text)
		// we're in, so those markers get injected at the same points the old
		// single-shot flush used to. claude/codex never set thinkingDelta/textDelta,
		// so streamBuf stays empty and neither helper below ever writes for them.
		let streamBuf = "";
		let streamSection: "none" | "thinking" | "text" = "none";
		let streamFinalized = false;
		let killTimer: ReturnType<typeof setTimeout> | null = null;

		// Two independent reapers, both landing the same `timedOut` outcome (the
		// TUI matches "timed out" verbatim — see selectors.rs — so the two paths
		// must not be distinguishable downstream):
		//  - idleTimer: the PRIMARY reaper. Reset on every parsed stream event in
		//    `handleLine`; fires when the worker has gone silent too long.
		//  - ceiling: a one-shot backstop equal to the resolved `timeoutMs`, so a
		//    run that keeps streaming (never idle) still cannot run forever.
		const killChild = () => {
			// Guard against both timers firing (idle and ceiling landing in the same
			// tick, or a redundant fire after the other already killed the child):
			// only the first call actually signals the process.
			if (timedOut) return;
			timedOut = true;
			if (child.pid) {
				try {
					process.kill(-child.pid, "SIGTERM");
				} catch {
					child.kill("SIGTERM");
				}
			}
			killTimer = setTimeout(() => {
				if (child.pid) {
					try {
						process.kill(-child.pid, "SIGKILL");
					} catch {}
				}
			}, 5000);
			killTimer.unref();
		};

		let idleTimer: ReturnType<typeof setTimeout> | null = setTimeout(
			killChild,
			idleTimeoutMs,
		);
		const ceiling = setTimeout(killChild, timeoutMs);

		const clearTimers = () => {
			if (idleTimer) clearTimeout(idleTimer);
			idleTimer = null;
			clearTimeout(ceiling);
			if (killTimer) clearTimeout(killTimer);
			if (promptFilePath) {
				try {
					unlinkSync(promptFilePath);
				} catch {
					// Best-effort: the run's outcome doesn't depend on cleanup succeeding.
				}
			}
		};

		// Flush any COMPLETE lines out of streamBuf: redact only whole lines so a
		// secret split across two token deltas (but landing within the same line)
		// is never redacted half-formed -- opts.redact only ever runs on text up to
		// and including the last "\n", never on a still-open partial line. Any
		// trailing partial line stays buffered for the next delta.
		const flushStreamLines = () => {
			const lastNl = streamBuf.lastIndexOf("\n");
			if (lastNl === -1) return;
			const complete = streamBuf.slice(0, lastNl + 1);
			streamBuf = streamBuf.slice(lastNl + 1);
			appendFileSync(opts.transcriptPath, opts.redact(complete));
		};

		// Append one thinking/text delta to the live stream, injecting the same
		// "### Thinking" header and blank-line separator the old batched flush used
		// -- but as each section STARTS rather than only once fully accumulated --
		// then flushing whatever complete lines that produced.
		const appendStreamDelta = (kind: "thinking" | "text", delta: string) => {
			if (streamSection === "none" && kind === "thinking") {
				streamBuf += "### Thinking\n";
			} else if (streamSection === "thinking" && kind === "text") {
				streamBuf += "\n\n";
			}
			streamSection = kind;
			streamBuf += delta;
			flushStreamLines();
		};

		// Flush whatever remains buffered (a trailing partial line with no newline
		// yet) plus the closing blank line, mirroring the tail of the old batched
		// shape. Idempotent so both the `result` event and the process `close`
		// handler can call it safely without double-writing; a no-op when nothing
		// was ever streamed (claude/codex, or a grok run with no deltas at all).
		const finalizeStreamBuffer = () => {
			if (streamFinalized) return;
			if (streamSection === "none") return;
			streamFinalized = true;
			streamBuf += "\n\n";
			appendFileSync(opts.transcriptPath, opts.redact(streamBuf));
			streamBuf = "";
		};

		const handleLine = (line: string) => {
			if (!line.trim()) return;
			let event: Record<string, unknown>;
			try {
				event = JSON.parse(line);
			} catch {
				return;
			}
			// A successfully-parsed event proves the worker is alive; reset the idle
			// window. Once a reaper has already fired (idleTimer nulled by
			// clearTimers), there's nothing to reset.
			if (idleTimer) {
				clearTimeout(idleTimer);
				idleTimer = setTimeout(killChild, idleTimeoutMs);
			}
			appendFileSync(opts.eventsPath, `${opts.redact(line)}\n`);

			const parsed = adapter.parseEvent(event);
			if (!sessionId && parsed.sessionId) {
				sessionId = parsed.sessionId;
			}
			if (parsed.thinkingDelta) {
				thinkingAcc += parsed.thinkingDelta;
				appendStreamDelta("thinking", parsed.thinkingDelta);
			}
			if (parsed.textDelta) {
				textAcc += parsed.textDelta;
				appendStreamDelta("text", parsed.textDelta);
			}
			if (parsed.result) {
				// Delta-stream adapters (grok) carry no full-text result event, so
				// fall back to the accumulated text; direct adapters set a non-empty
				// text and the `||` never flips.
				resultText = parsed.result.text || textAcc;
				usage = {
					costUsd: parsed.result.costUsd,
					turns: parsed.result.turns,
					durationMs: parsed.result.durationMs,
					inputTokens: parsed.result.inputTokens,
					outputTokens: parsed.result.outputTokens,
				};
				finalizeStreamBuffer();
			}
			if (parsed.transcriptMd) {
				appendFileSync(
					opts.transcriptPath,
					`${opts.redact(parsed.transcriptMd)}\n`,
				);
			}
		};

		child.stdout?.on("data", (chunk: Buffer) => {
			lineBuffer += chunk.toString();
			const lines = lineBuffer.split("\n");
			lineBuffer = lines.pop() ?? "";
			for (const line of lines) handleLine(line);
		});

		child.stderr?.on("data", (chunk: Buffer) => {
			stderr += chunk.toString();
		});

		child.on("close", (code, signal) => {
			clearTimers();
			if (lineBuffer) handleLine(lineBuffer);
			// Stream ended without a result event (crash/timeout): still land any
			// buffered stream content. No-op if the result event already finalized it.
			finalizeStreamBuffer();
			resolve({
				exitCode: code ?? 1,
				timedOut,
				signal: signal ?? null,
				sessionId,
				resultText,
				stderr,
				usage,
			});
		});

		child.on("error", () => {
			clearTimers();
			resolve({
				exitCode: 1,
				timedOut: false,
				signal: null,
				sessionId,
				resultText: "",
				stderr: "Failed to spawn process",
				usage,
			});
		});
	});
}

export function executeClaude(opts: ExecuteClaudeOptions): Promise<RunResult> {
	return executeRun(claudeAdapter, opts);
}

/** Trailing-output cap (chars) retained from a verify command's combined
 * stdout+stderr. The buffer is trimmed to this many trailing characters as it
 * streams, so a chatty check cannot balloon the daemon's memory; the caller
 * persists whatever tail is returned (~4 KB). */
export const VERIFY_OUTPUT_LIMIT = 4096;

export interface ExecuteVerifyOptions {
	command: string;
	cwd: string;
	timeoutMs: number;
	/** Retained trailing-output cap; defaults to [`VERIFY_OUTPUT_LIMIT`]. */
	outputLimit?: number;
}

export interface VerifyResult {
	exitCode: number;
	timedOut: boolean;
	/** Signal that killed the child (e.g. "SIGTERM" on the timeout path), else null. */
	signal: string | null;
	/** Combined stdout+stderr in arrival order, trimmed to the trailing
	 * `outputLimit` characters. */
	output: string;
}

/**
 * Run a done-condition (`verify`) command via `/bin/bash -lc` in `cwd`, mirroring
 * [`executeClaude`]'s timeout→SIGTERM→SIGKILL detached-group kill. stdout and
 * stderr fold into ONE tail-bounded buffer (a verify is meant to be a short
 * check; a runaway one must not OOM the daemon). Never rejects — a spawn failure
 * resolves as a non-zero exit so the caller lands `verify-failed` instead of
 * crashing the worker. This is the sanctioned process spawn (see AGENTS.md: the
 * child spawn lives in runner.ts).
 */
export function executeVerify(
	opts: ExecuteVerifyOptions,
): Promise<VerifyResult> {
	const timeoutMs = Math.max(1000, opts.timeoutMs);
	const limit = opts.outputLimit ?? VERIFY_OUTPUT_LIMIT;
	return new Promise((resolve) => {
		let child: ChildProcess;
		try {
			child = spawn("/bin/bash", ["-lc", opts.command], {
				cwd: opts.cwd,
				stdio: ["ignore", "pipe", "pipe"],
				detached: true,
			});
		} catch (err) {
			resolve({
				exitCode: 1,
				timedOut: false,
				signal: null,
				output: err instanceof Error ? err.message : String(err),
			});
			return;
		}

		let output = "";
		let timedOut = false;
		let killTimer: ReturnType<typeof setTimeout> | null = null;
		const append = (chunk: Buffer) => {
			output += chunk.toString();
			// Keep only the trailing window so a noisy command stays bounded.
			if (output.length > limit) output = output.slice(-limit);
		};

		const timeout = setTimeout(() => {
			timedOut = true;
			if (child.pid) {
				try {
					process.kill(-child.pid, "SIGTERM");
				} catch {
					child.kill("SIGTERM");
				}
			}
			killTimer = setTimeout(() => {
				if (child.pid) {
					try {
						process.kill(-child.pid, "SIGKILL");
					} catch {}
				}
			}, 5000);
			killTimer.unref();
		}, timeoutMs);

		child.stdout?.on("data", append);
		child.stderr?.on("data", append);

		child.on("close", (code, signal) => {
			clearTimeout(timeout);
			if (killTimer) clearTimeout(killTimer);
			resolve({
				exitCode: code ?? 1,
				timedOut,
				signal: signal ?? null,
				output,
			});
		});

		child.on("error", () => {
			clearTimeout(timeout);
			if (killTimer) clearTimeout(killTimer);
			resolve({
				exitCode: 1,
				timedOut,
				signal: null,
				output: output || "Failed to spawn verify process",
			});
		});
	});
}
