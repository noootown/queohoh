import type { TaskDefinition } from "./definition.js";
import { execHook } from "./hooks.js";
import { resolveModel } from "./models.js";
import type { Redactor } from "./redact.js";
import type { Exec } from "./resolver-io.js";
import type { RunStore } from "./run-store.js";
import type { executeClaude, executeVerify, RunResult } from "./runner.js";
import type { SessionLineageStore } from "./session-lineage.js";
import type { QueueStore } from "./store.js";
import type { TaskInstance } from "./task.js";
import { laneKey } from "./task.js";
import { render } from "./template.js";
import { extractTicket } from "./worktree-context.js";

export type ClaudeExecutor = typeof executeClaude;
export type VerifyExecutor = typeof executeVerify;

/** Default ceiling for a `verify` (done-condition) command: 10 minutes. A check
 * is meant to be quick; this only guards a wedged one (it lands `verify-failed`).
 * A constant rather than a per-definition knob — no definition has needed to
 * override it, and the run's own `timeout` already bounds the worker itself. */
export const VERIFY_TIMEOUT_MS = 600_000;

/** Matches Claude's own "you've hit your session/usage limit" message (e.g.
 * `You've hit your session limit · resets 1pm (America/Chicago)`). Permissive
 * on the noun (session/usage) and the surrounding wording, since the exact
 * phrasing isn't a stable API contract — a false negative just falls back to
 * the generic `exit code N` reason, never a crash. */
export const SESSION_LIMIT_RE = /\b(?:session|usage)\s+limit\b/i;

export interface WorkerDeps {
	store: QueueStore;
	runStore: RunStore;
	exec: Exec;
	executeClaude: ClaudeExecutor;
	redact: Redactor;
	loadDef: (definition: string) => TaskDefinition | null;
	worktreePath: (repo: string, worktree: string) => Promise<string | null>;
	defaults: { model: string; timeoutMs: number };
	globalVars?: Record<string, string>;
	repoVars?: Record<string, string>;
	/** Effective alias→id table for the task's repo; absent = no resolution
	 * (old callers). Already merged (defaults ⊕ global ⊕ project). */
	modelTable?: Record<string, string>;
	lineage?: SessionLineageStore;
	// Reports the spawned claude child's pid so the engine can track it for a
	// Stop action. Fires once per run, right after spawn.
	onSpawned?: (taskId: string, pid: number) => void;
	// True when the engine has recorded a user Stop for this task, so a kill
	// signal settles as `cancelled` (user-intentional) rather than `failed`. A
	// signal WITHOUT this flag (e.g. an external/OOM kill) still means failed.
	isCancelled?: (taskId: string) => boolean;
	// Runs the task's done-condition (`verify`) command after a successful run.
	// Optional (like mainSessions/onSpawned): absent = the gate is skipped even
	// when a verify command is configured, so a caller that never wires it keeps
	// today's behavior. The daemon always injects it.
	executeVerify?: VerifyExecutor;
}

const EMPTY_RESULT: RunResult = {
	exitCode: 1,
	timedOut: false,
	signal: null,
	sessionId: null,
	resultText: "",
	stderr: "",
	usage: { costUsd: null, turns: null, durationMs: null },
};

export async function runTask(
	taskId: string,
	deps: WorkerDeps,
): Promise<TaskInstance> {
	const task = deps.store.get(taskId);
	if (!task) throw new Error(`task not found: ${taskId}`);
	// A (re-)run clears the previous verify verdict; it is re-stamped only if this
	// run reaches the verify gate below. `verify` (the configured command) is left
	// untouched — it is configuration, not a per-run result.
	deps.store.update(taskId, {
		status: "running",
		error: null,
		verified: null,
		verifyExitCode: null,
		verifyOutput: null,
	});

	// Item vars ARE available at run time; precedence global < repo < item.
	const globalVars = deps.globalVars ?? {};
	const repoVars = deps.repoVars ?? {};

	const fail = (reason: string, result: RunResult = EMPTY_RESULT) => {
		deps.runStore.finishRun(
			taskId,
			{ result, outcome: "failed", reason },
			deps.redact,
		);
		return deps.store.update(taskId, { status: "failed", error: reason });
	};

	const worktree = task.target.worktree;
	if (worktree === null) {
		return fail("worktree path not found: unresolved task");
	}
	const cwd = await deps.worktreePath(task.target.repo, worktree);
	if (cwd === null) {
		return fail(`worktree path not found: ${laneKey(task)}`);
	}

	// Execution-time worktree context. Every task runs in a resolved worktree,
	// so definitions can reference these without declaring args. The branch read
	// goes through the same exec seam as the dirty-tree guard below; a non-zero
	// exit or a throw leaves `branch` (and thus `ticket`) empty — never crashes.
	let branch = "";
	try {
		const head = await deps.exec("git", ["rev-parse", "--abbrev-ref", "HEAD"], {
			cwd,
		});
		if (head.exitCode === 0) branch = head.stdout.trim();
	} catch {
		branch = "";
	}
	const worktreeContext: Record<string, string> = {
		worktree,
		worktree_path: cwd,
		branch,
		ticket: extractTicket(branch),
	};

	// Hooks see the worktree context at LOWEST precedence: it spreads before the
	// explicit global vars so an explicitly configured `branch`/`ticket`/etc.
	// wins over the worktree-derived value.
	const renderHook = (cmd: string) =>
		render(
			cmd,
			{ ...worktreeContext, ...globalVars },
			repoVars,
			task.item ?? {},
		);

	let def: TaskDefinition | null = null;
	if (task.definition !== null) {
		def = deps.loadDef(task.definition);
		if (def === null) return fail(`definition not found: ${task.definition}`);
	}
	const model = resolveModel(
		def?.model ?? task.model ?? deps.defaults.model,
		deps.modelTable ?? {},
	);
	// Precedence: definition's own `timeout:` > per-task override (ad-hoc/chain
	// step, set via the MCP `timeout` param) > daemon default. Mirrors `model`
	// immediately above.
	const timeoutMs = def?.timeoutMs ?? task.timeoutMs ?? deps.defaults.timeoutMs;

	deps.runStore.writeSnapshot(
		taskId,
		{
			task,
			definition: def,
			resolvedWorktree: worktree,
			resolvedWorktreePath: cwd,
			prompt: task.prompt,
			model,
		},
		deps.redact,
	);
	deps.runStore.writeWorkerPid(taskId, process.pid);

	let outcome: "done" | "failed" | "cancelled" | "verify-failed" = "done";
	let reason: string | null = null;
	let result: RunResult = EMPTY_RESULT;
	// Populated only when the verify gate below actually runs; drives both the
	// persisted task fields and the run-store report/data.
	let verifyRun: {
		command: string;
		verified: boolean;
		exitCode: number | null;
		output: string;
	} | null = null;

	// pre_run
	let preRunOk = true;
	if (def?.preRun) {
		try {
			await execHook(renderHook(def.preRun), deps.exec, { cwd });
		} catch (err) {
			preRunOk = false;
			outcome = "failed";
			reason = `pre_run failed: ${err instanceof Error ? err.message : String(err)}`;
		}
	}

	// Resume resolution at SPAWN time. A pinned task resumes the TIP of its
	// pin's lineage: each headless resume of X mints a new session id (the
	// fork is recorded after the run below), so following the chain makes
	// queued follow-ups stack — without hijacking a task pinned to a
	// different session in the same lane. `session: "main"` is deprecated
	// and intentionally resolves nothing (fresh).
	let resumeSessionId: string | undefined;
	if (task.resumeSessionId !== null) {
		resumeSessionId =
			deps.lineage?.tip(task.resumeSessionId) ?? task.resumeSessionId;
	}

	// claude
	if (preRunOk) {
		result = await deps.executeClaude({
			// Second render pass at execution time: fills late worktree-context
			// refs the instantiate-time pass left literal. Only these vars are the
			// item layer; any other unknown `{{key}}` stays verbatim.
			prompt: render(task.prompt, {}, {}, worktreeContext),
			model,
			cwd,
			timeoutMs,
			resumeSessionId,
			eventsPath: deps.runStore.eventsPath(taskId),
			transcriptPath: deps.runStore.transcriptPath(taskId),
			redact: deps.redact,
			onSpawned: (pid) => deps.onSpawned?.(taskId, pid),
		});
		// Reason precedence: a timeout is its own outcome; else a signal (a Stop
		// kills the process group) wins over exit code, since a stopped run's
		// signal is the truer cause; else a non-zero exit.
		if (result.timedOut) {
			outcome = "failed";
			reason = "timed out";
		} else if (result.signal !== null) {
			// A kill signal (timeout already handled above). If the engine recorded
			// a user Stop for this task, it's a deliberate cancel — not a failure;
			// any other signal (external/OOM kill) is still a genuine failure.
			if (deps.isCancelled?.(taskId)) {
				outcome = "cancelled";
				reason = "stopped by user";
			} else {
				outcome = "failed";
				reason = `stopped (${result.signal})`;
			}
		} else if (result.exitCode !== 0) {
			outcome = "failed";
			// Claude's own session/usage-limit message lands in `resultText` with a
			// generic non-zero exit — no distinct exit code or event field marks it.
			// Stamping the terse "session limit" reason (matched verbatim, not
			// re-derived, by the TUI's glyph selection) lets the queue/worktree panes
			// show a distinct icon instead of the generic failed ✗, since retrying
			// immediately won't help (the fix is to wait for the reset).
			reason = SESSION_LIMIT_RE.test(result.resultText)
				? "session limit"
				: `exit code ${result.exitCode}`;
		}

		// Done-condition (`verify`) gate — the framework's own success check.
		// Runs ONLY when the run otherwise succeeded (still `done`). The command
		// comes from the definition (read live, like model/timeout/hooks) or the
		// task's own `verify` (ad-hoc/chain step), rendered with the same context
		// as the hooks. A non-zero exit or a timeout lands `verify-failed` —
		// distinct from `failed` so "the worker errored" reads differently from
		// "the worker claimed success but the check disagreed". There is
		// deliberately NO universal dirty-tree check anymore: it punished
		// `worktree: repo` tasks for pre-existing dirt in the user's own checkout;
		// defs that want it (autofix, pr-ready) declare it as their `verify`.
		const verifyCmd = def?.verify ?? task.verify ?? null;
		if (outcome === "done" && verifyCmd !== null && deps.executeVerify) {
			const v = await deps.executeVerify({
				command: renderHook(verifyCmd),
				cwd,
				timeoutMs: VERIFY_TIMEOUT_MS,
			});
			const passed = !v.timedOut && v.exitCode === 0;
			verifyRun = {
				command: verifyCmd,
				verified: passed,
				exitCode: v.timedOut ? null : v.exitCode,
				output: v.output,
			};
			if (!passed) {
				outcome = "verify-failed";
				reason = v.timedOut
					? "verify timed out"
					: `verify failed (exit ${v.exitCode})`;
			}
		}
	}

	// post_run — always attempted; its failure never flips a done outcome
	if (def?.postRun) {
		try {
			await execHook(renderHook(def.postRun), deps.exec, { cwd });
		} catch (err) {
			const msg = `post_run failed: ${err instanceof Error ? err.message : String(err)}`;
			console.error(`[queohoh] ${msg}`);
			reason = reason ? `${reason}; ${msg}` : null;
		}
	}

	// Record the fork after any outcome (done OR failed): resuming
	// `resumeSessionId` produced `result.sessionId`, so future pins anywhere
	// on this chain resolve to the new tip. Fresh runs record nothing —
	// their session becomes a lineage root for future picks.
	if (
		resumeSessionId !== undefined &&
		deps.lineage &&
		result.sessionId !== null &&
		result.sessionId !== resumeSessionId
	) {
		deps.lineage.recordFork(resumeSessionId, result.sessionId);
	}

	deps.runStore.finishRun(
		taskId,
		{ result, outcome, reason, verify: verifyRun },
		deps.redact,
	);
	return deps.store.update(taskId, {
		status: outcome,
		// `done` clears the error; failed/cancelled/verify-failed carry their reason
		// (the detail pane shows "stopped by user" for a cancel).
		error: outcome === "done" ? null : reason,
		// Stamp the verify verdict when the gate ran. `verify` records the command
		// that was checked (for a definition task this stamps the definition's
		// command onto the record); the output tail is redacted before it lands in
		// the task file (which is not otherwise passed through the redactor).
		...(verifyRun && {
			verify: verifyRun.command,
			verified: verifyRun.verified,
			verifyExitCode: verifyRun.exitCode,
			verifyOutput: deps.redact(verifyRun.output),
		}),
	});
}
