import { type ChildProcess, spawn } from "node:child_process";
import { appendFileSync, writeFileSync } from "node:fs";
import type { Redactor } from "./redact.js";

export interface RunUsage {
	costUsd: number | null;
	turns: number | null;
	durationMs: number | null;
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
}

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

export function executeClaude(opts: ExecuteClaudeOptions): Promise<RunResult> {
	const timeoutMs = Math.max(1000, opts.timeoutMs);
	const args = [
		"-p",
		opts.prompt,
		"--output-format",
		"stream-json",
		"--verbose",
		"--model",
		opts.model,
		...(opts.resumeSessionId ? ["--resume", opts.resumeSessionId] : []),
		...(opts.claudeArgs ?? []),
	];

	return new Promise((resolve) => {
		// Initialize run files BEFORE spawning so a failure (e.g. missing parent
		// dir) never orphans a detached child. On failure, resolve without spawning.
		try {
			writeFileSync(opts.eventsPath, "");
			writeFileSync(opts.transcriptPath, "");
		} catch (err) {
			const msg = err instanceof Error ? err.message : String(err);
			resolve({
				exitCode: 1,
				timedOut: false,
				signal: null,
				sessionId: null,
				resultText: "",
				stderr: `Failed to initialize run files: ${msg}`,
				usage: { costUsd: null, turns: null, durationMs: null },
			});
			return;
		}

		const child: ChildProcess = spawn(opts.claudeBin ?? "claude", args, {
			env: { ...process.env, ...opts.env },
			cwd: opts.cwd,
			stdio: ["ignore", "pipe", "pipe"],
			detached: true,
		});
		if (child.pid && opts.onSpawned) opts.onSpawned(child.pid);

		let stderr = "";
		let resultText = "";
		let timedOut = false;
		let sessionId: string | null = null;
		let usage: RunUsage = { costUsd: null, turns: null, durationMs: null };
		let lineBuffer = "";
		let killTimer: ReturnType<typeof setTimeout> | null = null;

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

		const handleLine = (line: string) => {
			if (!line.trim()) return;
			let event: Record<string, unknown>;
			try {
				event = JSON.parse(line);
			} catch {
				return;
			}
			appendFileSync(opts.eventsPath, `${opts.redact(line)}\n`);

			if (!sessionId && event.session_id) {
				sessionId = event.session_id as string;
			}
			if ((event.type as string) === "result") {
				resultText = (event.result as string) ?? "";
				usage = {
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
			if (md) appendFileSync(opts.transcriptPath, `${opts.redact(md)}\n`);
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
			clearTimeout(timeout);
			if (killTimer) clearTimeout(killTimer);
			if (lineBuffer) handleLine(lineBuffer);
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
			clearTimeout(timeout);
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
