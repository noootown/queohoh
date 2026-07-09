import type { TaskDefinition } from "./definition.js";
import { execHook } from "./hooks.js";
import type { MainSessionStore } from "./main-sessions.js";
import type { Redactor } from "./redact.js";
import type { Exec } from "./resolver-io.js";
import type { RunStore } from "./run-store.js";
import type { executeClaude, RunResult } from "./runner.js";
import type { QueueStore } from "./store.js";
import type { TaskInstance } from "./task.js";
import { laneKey } from "./task.js";
import { render } from "./template.js";
import { extractTicket } from "./worktree-context.js";

export type ClaudeExecutor = typeof executeClaude;

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
	mainSessions?: MainSessionStore;
}

const EMPTY_RESULT: RunResult = {
	exitCode: 1,
	timedOut: false,
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
	deps.store.update(taskId, { status: "running", error: null });

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
	const model = def?.model ?? deps.defaults.model;
	const timeoutMs = def?.timeoutMs ?? deps.defaults.timeoutMs;

	deps.runStore.writeSnapshot(
		taskId,
		{
			task,
			definition: def,
			resolvedWorktree: worktree,
			prompt: task.prompt,
			model,
		},
		deps.redact,
	);
	deps.runStore.writeWorkerPid(taskId, process.pid);

	let outcome: "done" | "failed" = "done";
	let reason: string | null = null;
	let result: RunResult = EMPTY_RESULT;

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

	// Main-session pointer: resolved at SPAWN time. laneKey is null only when the
	// worktree is unresolved (guarded above), in which case we treat as fresh.
	const lane = laneKey(task);
	let resumeSessionId: string | undefined;
	if (task.session === "main" && deps.mainSessions && lane !== null) {
		resumeSessionId = deps.mainSessions.get(lane) ?? undefined;
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
		});
		if (result.timedOut) {
			outcome = "failed";
			reason = "timed out";
		} else if (result.exitCode !== 0) {
			outcome = "failed";
			reason = `exit code ${result.exitCode}`;
		} else {
			const status = await deps.exec("git", ["status", "--porcelain"], { cwd });
			if (status.exitCode !== 0 || status.stdout.trim() !== "") {
				outcome = "failed";
				reason = "tree left dirty";
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

	// Advance the pointer after any outcome (done OR failed) when a main run
	// captured a sessionId; runs with a null sessionId leave it unchanged.
	if (
		task.session === "main" &&
		deps.mainSessions &&
		lane !== null &&
		result.sessionId !== null
	) {
		deps.mainSessions.set(lane, result.sessionId);
	}

	deps.runStore.finishRun(taskId, { result, outcome, reason }, deps.redact);
	return deps.store.update(taskId, {
		status: outcome,
		error: outcome === "failed" ? reason : null,
	});
}
