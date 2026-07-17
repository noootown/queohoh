import {
	type CatalogEntry,
	findModel,
	groupHead,
	modelRef,
} from "./catalog.js";
import { DEFAULT_PROVIDERS, type ProviderConfig } from "./config.js";
import type { TaskDefinition } from "./definition.js";
import { execHook } from "./hooks.js";
import { type ChainEntry, resolveModelChain } from "./models.js";
import { getAdapter } from "./providers/index.js";
import type { Redactor } from "./redact.js";
import type { Exec } from "./resolver-io.js";
import type { RunStore, SpawnSpec } from "./run-store.js";
import type { executeClaude, executeVerify, RunResult } from "./runner.js";
import { executeRun } from "./runner.js";
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

export interface WorkerDeps {
	store: QueueStore;
	runStore: RunStore;
	exec: Exec;
	executeClaude: ClaudeExecutor;
	redact: Redactor;
	loadDef: (definition: string) => TaskDefinition | null;
	worktreePath: (repo: string, worktree: string) => Promise<string | null>;
	/** Daemon default wall-clock ceiling; the default MODEL leg is gone — a
	 * task with no `model:` of its own resolves against `defaultModels` (below)
	 * via `resolveModelChain`, not a single default alias. */
	defaults: { timeoutMs: number };
	globalVars?: Record<string, string>;
	repoVars?: Record<string, string>;
	/** Effective model catalog for the task's repo (BUILTIN_CATALOG ⊕ config
	 * `catalog:` overlay), already layered by the caller (the engine, Task 5).
	 * Every `provider/label` ref a task/definition names resolves against this.
	 * See `resolveModelChain` / `effectiveCatalog`. */
	catalog: CatalogEntry[];
	/** Ordered fallback model-ref list a task/definition with NO `model:` of its
	 * own resolves against (design spec §2), already project-resolved by the
	 * caller. Passed to `resolveModelChain` verbatim: an empty list is NOT
	 * special-cased here — with an enabled `activeProvider`, `resolveModelChain`
	 * still heads a null-model task onto that provider's group head, so the task
	 * stays runnable. The empty-array-means-unset → global fallback is the
	 * engine's decision at wiring time (Task 5), not the worker's. */
	defaultModels: string[];
	/** Effective provider table (fallback order), already layered by the
	 * caller (built-in ⊕ global config.yaml ⊕ project vars.yaml — see
	 * `effectiveProviders` in config.ts). Absent ⇒ `DEFAULT_PROVIDERS` (old
	 * callers, or a caller that hasn't wired provider config yet). */
	providers?: ProviderConfig[];
	/** Which provider the operator has currently switched to (design spec §4
	 * chain resolution's `activeProvider`). Required: it re-heads the fallback
	 * chain onto the switched-to provider for a fresh run and is ignored by a
	 * resume pin (which follows its session's tagged provider). */
	activeProvider: string;
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
	/** Adapter name for the chain's head entry — what actually spawns. */
	provider: string;
	systemPrompt: string | undefined;
	extraArgs: string[] | undefined;
	bin: string | undefined;
	/** The head entry's `provider/label` ref — what the hop-trail / terminal
	 * "attempt N: <ref>" line reports (design spec §4). Distinct from the bare
	 * `provider` name appended to `attemptedModels` (the group-skip key). */
	ref: string;
	/** Remaining fallback chain (attemptedModels already filtered out; for
	 * a resume task, pinned to the single provider its session lineage is
	 * tagged with — see the resume-resolution block in `resolveRunContext`).
	 * `chain[0]` is always the entry that produced `provider`/`model` above;
	 * `chain.length > 1` is what `finalizeRun` checks to decide whether an
	 * availability failure has somewhere left to fall back to. */
	chain: ChainEntry[];
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

	// Chain resolution (design spec §4). `deps.providers`/`deps.catalog`/
	// `deps.defaultModels` are already the effective (built-in ⊕ global ⊕
	// project) tables — the caller's (engine, Task 5) job, not this function's.
	// A task/definition with no `model:` of its own passes `null`, so the chain
	// comes from `deps.defaultModels` (headed onto `activeProvider`).
	//
	// Precedence is task.model first: a TUI def-run exact pick (or enqueue)
	// stamps an override on the task and must beat the def's authored list.
	// Without a stamp, fall through to def.model, then default_models.
	const providers: ProviderConfig[] = deps.providers ?? DEFAULT_PROVIDERS;
	const modelSpec = task.model ?? def?.model ?? null;
	const chainResult = resolveModelChain(
		modelSpec,
		deps.catalog,
		providers,
		deps.defaultModels,
		deps.activeProvider,
	);
	if (!chainResult.ok) return { fail: chainResult.error };

	// Drop entries already attempted for this task, so a retry (or an adopted
	// mid-chain run after a daemon restart) resolves onto the next candidate
	// rather than repeating the one that just failed. `attemptedModels` holds
	// bare PROVIDER names (an availability failure marks the whole provider
	// group attempted — see finalizeRun), so the `e.provider` clause skips
	// every entry of an attempted provider (provider-group skip); the `e.ref`
	// clause is future-proofing for a per-ref attempt entry, and both together
	// also honor a legacy `attempted_providers` file mapped onto attemptedModels.
	let chain = chainResult.chain.filter(
		(e) =>
			!task.attemptedModels.includes(e.ref) &&
			!task.attemptedModels.includes(e.provider),
	);

	// Resume resolution. A pinned task resumes the TIP of its pin's lineage:
	// each headless resume of X mints a new session id (the fork is recorded
	// after the run), so following the chain makes queued follow-ups stack —
	// without hijacking a task pinned to a different session in the same lane.
	// `session: "main"` is deprecated and intentionally resolves nothing (fresh).
	let resumeSessionId: string | undefined;
	if (task.resumeSessionId !== null) {
		resumeSessionId =
			deps.lineage?.tip(task.resumeSessionId) ?? task.resumeSessionId;

		// A resume task never falls back (spec decision 2) — its context lives
		// in a specific provider's session, so the chain is pinned to exactly
		// that provider (untagged/unknown sessions default to claude, matching
		// pre-Task-11 behavior for lineage files that predate provider tags).
		const pinnedProvider =
			deps.lineage?.providerOf(resumeSessionId) ?? "claude";
		const pinnedEntry = chain.find((e) => e.provider === pinnedProvider);
		if (pinnedEntry !== undefined) {
			chain = [pinnedEntry];
		} else {
			// The pinned provider isn't among the entries resolveModelChain
			// produced for this task's model spec (e.g. the spec names only some
			// OTHER provider). Resolve its single entry from the catalog directly:
			// the task's own ref when that ref names the pinned provider, else the
			// pinned provider's group head (its most powerful model). A disabled
			// provider — or one with no catalog entry at all — is unavailable.
			const pinnedConfig = providers.find((p) => p.name === pinnedProvider);
			if (pinnedConfig === undefined || !pinnedConfig.enabled) {
				return {
					fail: `resume provider unavailable: ${pinnedProvider}`,
				};
			}
			const specRef = typeof modelSpec === "string" ? modelSpec : null;
			const refEntry =
				specRef !== null ? findModel(deps.catalog, specRef) : undefined;
			const pinned =
				refEntry !== undefined && refEntry.provider === pinnedProvider
					? refEntry
					: groupHead(deps.catalog, pinnedProvider);
			if (pinned === undefined) {
				return {
					fail: `resume provider unavailable: ${pinnedProvider}`,
				};
			}
			chain = [
				{
					provider: pinned.provider,
					model: pinned.id,
					ref: modelRef(pinned),
				},
			];
		}
	}
	const head = chain[0];
	if (head === undefined) {
		return { fail: "no provider available: all providers attempted" };
	}
	const providerConfig = providers.find((p) => p.name === head.provider);

	// Precedence: definition's own `timeout:` > per-task override (ad-hoc/chain
	// step, set via the MCP `timeout` param) > daemon default. Unlike `model`
	// (task beats def so TUI/enqueue overrides win), timeout keeps def-first.
	const timeoutMs = def?.timeoutMs ?? task.timeoutMs ?? deps.defaults.timeoutMs;

	return {
		ctx: {
			task,
			cwd,
			worktree,
			worktreeContext,
			def,
			renderHook,
			model: head.model,
			timeoutMs,
			resumeSessionId,
			provider: head.provider,
			ref: head.ref,
			systemPrompt: providerConfig?.systemPrompt,
			extraArgs: providerConfig?.args,
			bin: providerConfig?.bin,
			chain,
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

	// A chain hop's (or a user rerun's) PRIOR attempt may have left a
	// result.json behind — only the shim ever unlinks spawn.json, nothing
	// unlinks result.json. Left in place, a daemon restart during THIS attempt
	// would let the adoption sweep's `hasResult → "finalize"` check finalize the
	// task with the stale result while this attempt's shim is still running.
	// Cleared before any of this attempt's own artifacts are written.
	deps.runStore.clearResultJson(taskId);

	deps.runStore.writeSnapshot(
		taskId,
		{
			task: ctx.task,
			definition: ctx.def,
			resolvedWorktree: ctx.worktree,
			resolvedWorktreePath: ctx.cwd,
			prompt: ctx.task.prompt,
			model: ctx.model,
			provider: ctx.provider,
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
		provider: ctx.provider,
		systemPrompt: ctx.systemPrompt,
		extraArgs: ctx.extraArgs,
		bin: ctx.bin,
	};
	return { kind: "spawn", spec };
}

/** `finalizeRun`'s outcome: the settled (or re-queued, see `retry`) task, plus
 * whether this was an availability failure with chain remaining. `retry:
 * true` means the run is NOT settled — the task was stamped back to `queued`
 * with the failed provider appended to `attemptedModels` — and the caller
 * must start a fresh attempt (the engine re-drives; `runTask`'s in-process
 * caller just invokes `runTask` again) rather than treat this as terminal. */
export interface FinalizeOutcome {
	task: TaskInstance;
	retry: boolean;
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
): Promise<FinalizeOutcome> {
	const c = await resolveRunContext(taskId, deps);
	if ("fail" in c) {
		// Defensive — should not happen post-spawn (the same context resolved
		// cleanly moments ago to produce the SpawnSpec).
		deps.runStore.finishRun(
			taskId,
			{ result: EMPTY_RESULT, outcome: "failed", reason: c.fail },
			deps.redact,
		);
		const task = deps.store.update(taskId, {
			status: "failed",
			error: c.fail,
		});
		return { task, retry: false };
	}
	const { task, cwd, renderHook, def, resumeSessionId, provider, ref, chain } =
		c.ctx;

	let outcome: "done" | "failed" | "cancelled" | "verify-failed" = "done";
	let reason: string | null = null;
	// Availability failure (session limit / out of budget / provider
	// unavailable) with a next chain entry to try and no resume pin holding it
	// on this provider. Set only inside the exit-code branch below.
	let retry = false;
	// True whenever `reason` holds a NORMALIZED availability reason
	// (classifyUnavailable's vocabulary, including the spawn-failure fallback
	// below) rather than a raw `exit code N`. Drives whether a TERMINAL
	// availability settle (chain exhausted, or a resume pin that can never
	// retry) still appends `provider` to `attemptedModels` — `retry` alone
	// only covers the hop-eligible case (finding 4).
	let availabilityClassified = false;
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
		reason = `exit code ${result.exitCode}`;
		// A missing/unspawnable provider binary (ENOENT et al.) or a run-file-init
		// failure never reaches any adapter's own wording — it's the RUNNER's
		// generic `child.on("error")`/pre-spawn message (runner.ts), not the CLI's
		// stderr. Checked before the adapter so a missing bin still falls through
		// the chain instead of settling as a generic failure (spec §2 shared
		// behaviors; finding 2).
		const spawnFailed =
			result.stderr.startsWith("Failed to spawn process") ||
			result.stderr.startsWith("Failed to initialize run files:");
		// The adapter's own availability wording (session/usage limit, out of
		// credits, quota/auth failures — see each provider's `classifyUnavailable`)
		// lands in `resultText`/`stderr` with a generic non-zero exit — no
		// distinct exit code or event field marks it. A non-null classification
		// overrides the generic reason (matched verbatim, not re-derived, by
		// the TUI's glyph selection) so the queue/worktree panes show a
		// distinct icon instead of the generic failed ✗ — AND, when nothing
		// pins this task to its current provider and the chain has somewhere
		// left to go, signals a retry on the next provider instead of settling
		// failed (spec §4 point 2). A resume task or an exhausted chain still
		// lands here with the classified reason, just terminal (point 3).
		const classified = spawnFailed
			? "provider unavailable"
			: (getAdapter(provider)?.classifyUnavailable({
					exitCode: result.exitCode,
					stderr: result.stderr,
					resultText: result.resultText,
				}) ?? null);
		if (classified !== null) {
			reason = classified;
			availabilityClassified = true;
			if (task.resumeSessionId === null && chain.length > 1) {
				retry = true;
			}
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

	// post_run — always attempted (per attempt, retry or not); its failure
	// never flips a done outcome
	if (def?.postRun) {
		try {
			await execHook(renderHook(def.postRun), deps.exec, { cwd });
		} catch (err) {
			const msg = `post_run failed: ${err instanceof Error ? err.message : String(err)}`;
			console.error(`[queohoh] ${msg}`);
			reason = reason ? `${reason}; ${msg}` : null;
		}
	}

	// Availability failure with somewhere left to fall back to: this attempt's
	// own report/data still lands in the run store (the "attempt 1: claude —
	// session limit" trail), but the TASK does not settle — it goes back to
	// `queued` with `provider` recorded so the next `startRun` resolves onto
	// the chain's next entry. No verify (outcome !== "done" already skipped
	// it above) and no lineage-fork recording (a retry-eligible failure only
	// ever happens on a fresh, non-resume run — see the `chain.length > 1`
	// gate above — so there is no fork to record here).
	if (retry) {
		// Log the hop (spec §4: "attempt 1: claude — session limit → falling
		// back to grok") BEFORE finishRun, so finishRun's own read of data.json
		// picks it up and renders it into THIS attempt's report.md — and so it's
		// on disk (via writeSnapshot's preserve-on-rewrite) before the next
		// attempt's startRun overwrites the rest of the snapshot.
		deps.runStore.appendAttempt(
			taskId,
			`attempt ${task.attemptedModels.length + 1}: ${ref} — ${reason} → falling back`,
			deps.redact,
		);
		deps.runStore.finishRun(
			taskId,
			{ result, outcome: "failed", reason, verify: null },
			deps.redact,
		);
		// Append the bare PROVIDER name (not the ref): an availability failure
		// marks the whole provider group attempted, so the next resolve skips
		// every entry of this provider (the group-skip filter above), not just
		// this one ref.
		const task2 = deps.store.update(taskId, {
			status: "queued",
			error: reason,
			attemptedModels: [...task.attemptedModels, provider],
		});
		return { task: task2, retry: true };
	}

	// Tag this run's session with the provider that actually produced it —
	// fresh run or resumed hop alike — so a later resume pinned to it (or to
	// a descendant, via `tip`) re-pins its fallback chain to this single
	// provider instead of re-walking the whole ladder from the top.
	if (deps.lineage && result.sessionId !== null) {
		deps.lineage.recordProvider(result.sessionId, provider);
	}

	// Record the fork after any outcome (done OR failed): resuming
	// `resumeSessionId` produced `result.sessionId`, so future pins anywhere
	// on this chain resolve to the new tip. A fresh run records no fork —
	// its session becomes a lineage root for future picks (it's still
	// provider-tagged above, just with no parent to link).
	if (
		resumeSessionId !== undefined &&
		deps.lineage &&
		result.sessionId !== null &&
		result.sessionId !== resumeSessionId
	) {
		deps.lineage.recordFork(resumeSessionId, result.sessionId);
	}

	// Terminal availability failure — chain exhausted (no next entry) or a
	// resume pin that can never hop (spec decision 2). `retry` above only
	// records the hop-eligible attempts in report.md's "## Attempts" trail, so
	// without this the trail silently drops the LAST provider tried (finding 4).
	// This ONLY appends to that report trail — it deliberately does NOT append
	// `provider` to the task's `attemptedModels`: that field records the
	// providers a manual re-run must skip (task.ts), and the terminal provider
	// is exactly the one a re-run should resume on (its rate limit may have
	// reset). Recording it here would filter it out too, so a chain-exhausted
	// task's retry would resolve onto an empty chain and fail "all providers
	// attempted" — see worker-fallback.test.ts's "no third hop recorded". Gated
	// on non-resume only so a resume task (whose report trail is a single pinned
	// provider) reads the same as before. No "→ falling back" suffix: there is
	// nowhere left to go.
	const terminalAvailabilityFailure =
		availabilityClassified && task.resumeSessionId === null;
	if (terminalAvailabilityFailure) {
		deps.runStore.appendAttempt(
			taskId,
			`attempt ${task.attemptedModels.length + 1}: ${ref} — ${reason}`,
			deps.redact,
		);
	}

	deps.runStore.finishRun(
		taskId,
		{ result, outcome, reason, verify: verifyRun },
		deps.redact,
	);
	const task2 = deps.store.update(taskId, {
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
	return { task: task2, retry: false };
}

/**
 * Dispatch a resolved `SpawnSpec` to the right in-process executor by
 * `spec.provider` — mirrors the daemon's `inProcessSpawner` (shim-host.ts):
 * `"claude"`/absent keeps going through `deps.executeClaude` (the injectable
 * seam tests fake to script a run's outcome); any other provider goes through
 * `executeRun` with its own adapter, mapping `spec.bin` onto `claudeBin` the
 * way `executeRun`'s options actually expect it. Before this, `runTask`
 * always called `deps.executeClaude` regardless of `spec.provider`, so a
 * chain hop onto e.g. grok would spawn claude with grok's model id and
 * `spec.bin` would silently no-op (finding 8) — invisible in production
 * because the daemon always spawns via the shim/`inProcessSpawner`, not this
 * function, but live for any direct embedder of `runTask`.
 */
async function spawnInProcess(
	taskId: string,
	spec: SpawnSpec,
	deps: WorkerDeps,
): Promise<RunResult> {
	const provider = spec.provider ?? "claude";
	const onSpawned = (pid: number) => deps.onSpawned?.(taskId, pid);
	if (provider === "claude") {
		return deps.executeClaude({ ...spec, redact: deps.redact, onSpawned });
	}
	const adapter = getAdapter(provider);
	// Defensive: resolveRunContext only ever produces a `provider` from the
	// effective providers table, whose entries are the registered adapters
	// (claude/codex/grok) — an unresolvable name here means the table and the
	// registry have drifted, not a normal runtime failure.
	if (!adapter) {
		return { ...EMPTY_RESULT, stderr: `unknown provider: ${provider}` };
	}
	return executeRun(adapter, {
		...spec,
		claudeBin: spec.bin,
		redact: deps.redact,
		onSpawned,
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
	// current process — `runTask` runs the executor itself, in-process (unlike
	// the daemon, which spawns a shim and records ITS pid instead).
	deps.runStore.writeWorkerPid(taskId, process.pid);
	const result = await spawnInProcess(taskId, s.spec, deps);
	// `retry: true` means finalizeRun already stamped the task back to
	// `queued` (with the failed provider appended to attemptedModels)
	// rather than settling it failed — return it as-is. The engine (Task 10)
	// is what actually re-drives a fresh attempt; here in the in-process path
	// the caller (a test, `qoo run`, ...) simply invokes `runTask` again.
	const outcome = await finalizeRun(taskId, result, deps);
	return outcome.task;
}
