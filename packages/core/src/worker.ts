import type { TaskDefinition } from "./definition.js";
import { execHook } from "./hooks.js";
import { resolveModel } from "./models.js";
import type { Redactor } from "./redact.js";
import type { Exec } from "./resolver-io.js";
import type { RunStore, SpawnSpec } from "./run-store.js";
import type { executeClaude, executeVerify, RunResult } from "./runner.js";
import type { SessionLineageStore } from "./session-lineage.js";
import type { QueueStore } from "./store.js";
import type { TaskInstance } from "./task.js";
import { laneKey } from "./task.js";
import { render } from "./template.js";
import { cleanCapturedOutput } from "./text.js";
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

/** Matches Anthropic's "you're out of credits/money" billing error (e.g.
 * `Your credit balance is too low to access the Anthropic API`). Distinct from
 * a session/usage limit: that resets on a timer, but this needs a top-up before
 * a rerun can succeed. Permissive on the exact wording (not a stable API
 * contract) — a false negative just falls back to the generic `exit code N`
 * reason, never a crash. Checked BEFORE `SESSION_LIMIT_RE` so the more specific
 * billing signal wins if both somehow appear. */
export const OUT_OF_BUDGET_RE =
	/credit balance (?:is )?too low|insufficient credits?|out of credits?/i;

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

/** Everything both `startRun` and `finalizeRun` need to know about a task's
 * run, resolved fresh each call (never cached): `finalizeRun` re-derives this
 * from disk rather than threading it through the spawn, so an adopted run
 * (re-derived after a daemon restart, with no in-memory state at all) goes
 * through the exact same resolution path as a live one. */
interface RunContext {
	task: TaskInstance;
	cwd: string;
	/** Resolved worktree name (task.target.worktree, non-null — resolveRunContext
	 * already failed the run otherwise). Kept alongside `worktreeContext` rather
	 * than re-read from it: `Record<string,string>` indexing is `string |
	 * undefined` under `noUncheckedIndexedAccess`, even for a key it always sets. */
	worktree: string;
	worktreeContext: Record<string, string>;
	def: TaskDefinition | null;
	renderHook: (cmd: string) => string;
	model: string;
	timeoutMs: number;
	resumeSessionId: string | undefined;
}

/** Shared pre/re-derivation for `startRun` and `finalizeRun`: resolves the
 * worktree cwd, the execution-time worktree context (branch/ticket), the
 * definition (if any), the effective model/timeout, and the resume session —
 * everything a run needs BEFORE and AFTER the spawn, without running any
 * hooks (pre_run/post_run stay the caller's responsibility, since only
 * `startRun` runs the former and only `finalizeRun` runs the latter). */
async function resolveRunContext(
	taskId: string,
	deps: WorkerDeps,
): Promise<{ ctx: RunContext } | { fail: string }> {
	const task = deps.store.get(taskId);
	if (!task) return { fail: `task not found: ${taskId}` };

	const worktree = task.target.worktree;
	if (worktree === null) {
		return { fail: "worktree path not found: unresolved task" };
	}
	const cwd = await deps.worktreePath(task.target.repo, worktree);
	if (cwd === null) {
		return { fail: `worktree path not found: ${laneKey(task)}` };
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

	// Item vars ARE available at run time; precedence global < repo < item.
	const globalVars = deps.globalVars ?? {};
	const repoVars = deps.repoVars ?? {};

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
		if (def === null)
			return { fail: `definition not found: ${task.definition}` };
	}
	const model = resolveModel(
		def?.model ?? task.model ?? deps.defaults.model,
		deps.modelTable ?? {},
	);
	// Precedence: definition's own `timeout:` > per-task override (ad-hoc/chain
	// step, set via the MCP `timeout` param) > daemon default. Mirrors `model`
	// immediately above.
	const timeoutMs = def?.timeoutMs ?? task.timeoutMs ?? deps.defaults.timeoutMs;

	// Resume resolution. A pinned task resumes the TIP of its pin's lineage:
	// each headless resume of X mints a new session id (the fork is recorded
	// after the run), so following the chain makes queued follow-ups stack —
	// without hijacking a task pinned to a different session in the same lane.
	// `session: "main"` is deprecated and intentionally resolves nothing (fresh).
	let resumeSessionId: string | undefined;
	if (task.resumeSessionId !== null) {
		resumeSessionId =
			deps.lineage?.tip(task.resumeSessionId) ?? task.resumeSessionId;
	}

	return {
		ctx: {
			task,
			cwd,
			worktree,
			worktreeContext,
			def,
			renderHook,
			model,
			timeoutMs,
			resumeSessionId,
		},
	};
}

/** Shared by `startRun`'s pre_run-failure path: post_run is ALWAYS attempted
 * (mirrors `finalizeRun`'s own post_run block) even though claude never ran,
 * then the run settles failed. Verbatim semantics of the pre-split single-flow
 * post_run block: its failure never overrides a null reason, and appends onto
 * a non-null one. */
async function settleFailedWithPostRun(
	taskId: string,
	ctx: RunContext,
	deps: WorkerDeps,
	initialReason: string,
): Promise<StartRunResult> {
	let reason: string | null = initialReason;
	if (ctx.def?.postRun) {
		try {
			await execHook(ctx.renderHook(ctx.def.postRun), deps.exec, {
				cwd: ctx.cwd,
			});
		} catch (err) {
			const msg = `post_run failed: ${err instanceof Error ? err.message : String(err)}`;
			console.error(`[queohoh] ${msg}`);
			reason = reason ? `${reason}; ${msg}` : null;
		}
	}
	deps.runStore.finishRun(
		taskId,
		{ result: EMPTY_RESULT, outcome: "failed", reason, verify: null },
		deps.redact,
	);
	deps.store.update(taskId, { status: "failed", error: reason });
	return { kind: "settled" };
}

/** `startRun`'s outcome: either the task already settled (pre-spawn failure —
 * nothing to spawn, the caller should just re-read the task) or a `SpawnSpec`
 * ready for `executeClaude`/the shim. */
export type StartRunResult =
	| { kind: "settled" }
	| { kind: "spawn"; spec: SpawnSpec };

/**
 * Pre-spawn half of a run: stamps `running`, resolves the run context, writes
 * the run-store snapshot, and runs `pre_run`. Never spawns claude itself (the
 * caller does, in-process via `runTask` or out-of-process via the shim) — this
 * lets the daemon persist the exact `SpawnSpec` to disk before handing it to a
 * detached shim, so a daemon crash between "spec built" and "shim launched"
 * never loses a run silently.
 */
export async function startRun(
	taskId: string,
	deps: WorkerDeps,
): Promise<StartRunResult> {
	// A (re-)run clears the previous verify verdict; it is re-stamped only if this
	// run reaches the verify gate below. `verify` (the configured command) is left
	// untouched — it is configuration, not a per-run result. `startedAt` is stamped
	// NOW (re-stamped on every re-run) so the live `⏱` timer counts from this run,
	// not the original creation — keeping it honest against the 3h wall-clock
	// ceiling, which the runner likewise measures fresh from this spawn.
	deps.store.update(taskId, {
		status: "running",
		startedAt: new Date().toISOString(),
		error: null,
		verified: null,
		verifyExitCode: null,
		verifyOutput: null,
	});

	const fail = (reason: string): StartRunResult => {
		deps.runStore.finishRun(
			taskId,
			{ result: EMPTY_RESULT, outcome: "failed", reason },
			deps.redact,
		);
		deps.store.update(taskId, { status: "failed", error: reason });
		return { kind: "settled" };
	};

	const c = await resolveRunContext(taskId, deps);
	if ("fail" in c) return fail(c.fail);
	const { ctx } = c;

	deps.runStore.writeSnapshot(
		taskId,
		{
			task: ctx.task,
			definition: ctx.def,
			resolvedWorktree: ctx.worktree,
			resolvedWorktreePath: ctx.cwd,
			prompt: ctx.task.prompt,
			model: ctx.model,
		},
		deps.redact,
	);

	// pre_run
	if (ctx.def?.preRun) {
		try {
			await execHook(ctx.renderHook(ctx.def.preRun), deps.exec, {
				cwd: ctx.cwd,
			});
		} catch (err) {
			const reason = `pre_run failed: ${err instanceof Error ? err.message : String(err)}`;
			return settleFailedWithPostRun(taskId, ctx, deps, reason);
		}
	}

	const spec: SpawnSpec = {
		// Second render pass at execution time: fills late worktree-context
		// refs the instantiate-time pass left literal. Only these vars are the
		// item layer; any other unknown `{{key}}` stays verbatim.
		prompt: render(ctx.task.prompt, {}, {}, ctx.worktreeContext),
		model: ctx.model,
		cwd: ctx.cwd,
		timeoutMs: ctx.timeoutMs,
		resumeSessionId: ctx.resumeSessionId,
		eventsPath: deps.runStore.eventsPath(taskId),
		transcriptPath: deps.runStore.transcriptPath(taskId),
	};
	return { kind: "spawn", spec };
}

/**
 * Post-spawn half of a run: classifies the `RunResult`, runs the `verify`
 * done-condition gate, runs `post_run`, records the session-lineage fork, and
 * persists the final report + task status. Re-derives its `RunContext` from
 * the persisted task rather than accepting one from the caller, so it works
 * identically whether called synchronously after `startRun` (`runTask`) or
 * minutes later against an adopted run the daemon never spawned itself.
 * Deliberately does NOT touch `status`/`startedAt` — those are `startRun`'s
 * job, and re-stamping them here would clobber the original start time of an
 * adopted run with "now".
 */
export async function finalizeRun(
	taskId: string,
	result: RunResult,
	deps: WorkerDeps,
): Promise<TaskInstance> {
	const c = await resolveRunContext(taskId, deps);
	if ("fail" in c) {
		// Defensive — should not happen post-spawn (the same context resolved
		// cleanly moments ago to produce the SpawnSpec).
		deps.runStore.finishRun(
			taskId,
			{ result: EMPTY_RESULT, outcome: "failed", reason: c.fail },
			deps.redact,
		);
		return deps.store.update(taskId, { status: "failed", error: c.fail });
	}
	const { task, cwd, renderHook, def, resumeSessionId } = c.ctx;

	let outcome: "done" | "failed" | "cancelled" | "verify-failed" = "done";
	let reason: string | null = null;
	// Populated only when the verify gate below actually runs; drives both the
	// persisted task fields and the run-store report/data.
	let verifyRun: {
		command: string;
		verified: boolean;
		exitCode: number | null;
		output: string;
	} | null = null;

	// Reason precedence: a recorded user Stop wins over everything, since it's
	// the most specific, deliberate signal — and Claude traps SIGTERM to clean
	// up its terminal, so a stopped run often exits by CODE (no signal) and
	// would otherwise fall through to the exit-code branch and masquerade as a
	// `failed` run. Else a timeout is its own outcome; else an unrequested kill
	// signal (external/OOM) wins over exit code, since it's the truer cause;
	// else a non-zero exit.
	if (deps.isCancelled?.(taskId)) {
		outcome = "cancelled";
		reason = "stopped by user";
	} else if (result.timedOut) {
		outcome = "failed";
		reason = "timed out";
	} else if (result.signal !== null) {
		// A kill signal we did NOT request (a user Stop is handled above): an
		// external/OOM kill is still a genuine failure.
		outcome = "failed";
		reason = `stopped (${result.signal})`;
	} else if (result.exitCode !== 0) {
		outcome = "failed";
		// Claude's own session/usage-limit AND credit-balance messages land in
		// `resultText` with a generic non-zero exit — no distinct exit code or
		// event field marks either. Stamping a terse reason (matched verbatim,
		// not re-derived, by the TUI's glyph selection) lets the queue/worktree
		// panes show a distinct icon instead of the generic failed ✗, since
		// retrying immediately won't help. Budget is checked first (more
		// specific: needs a top-up), then session limit (resets on a timer).
		if (OUT_OF_BUDGET_RE.test(result.resultText)) {
			reason = "out of budget";
		} else if (SESSION_LIMIT_RE.test(result.resultText)) {
			reason = "session limit";
		} else {
			reason = `exit code ${result.exitCode}`;
		}
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
			// Test runners emit ANSI colors + \r spinner overwrites; stored raw
			// they garble the TUI's cell renderer (see cleanCapturedOutput).
			output: cleanCapturedOutput(v.output),
		};
		if (!passed) {
			outcome = "verify-failed";
			reason = v.timedOut
				? "verify timed out"
				: `verify failed (exit ${v.exitCode})`;
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

/**
 * In-process composition of `startRun` + `executeClaude` + `finalizeRun` —
 * the whole run, synchronously awaited in this process. Callers that don't
 * need the daemon's detached-shim indirection (tests, `qoo run`, any
 * embedding of core directly) use this as before; the split only matters to
 * the daemon, which spawns the shim between the two halves instead.
 */
export async function runTask(
	taskId: string,
	deps: WorkerDeps,
): Promise<TaskInstance> {
	const s = await startRun(taskId, deps);
	if (s.kind === "settled") return deps.store.get(taskId) as TaskInstance;
	// Written AFTER startRun (not inside it): a pre_run failure never spawns
	// claude, so there is no pid to record. The pid recorded here is the
	// current process — `runTask` runs `executeClaude` itself, in-process
	// (unlike the daemon, which spawns a shim and records ITS pid instead).
	deps.runStore.writeWorkerPid(taskId, process.pid);
	const result = await deps.executeClaude({
		...s.spec,
		redact: deps.redact,
		onSpawned: (pid) => deps.onSpawned?.(taskId, pid),
	});
	return finalizeRun(taskId, result, deps);
}
