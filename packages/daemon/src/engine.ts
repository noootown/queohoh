import { execFileSync } from "node:child_process";
import type {
	ClaudeExecutor,
	Exec,
	GlobalConfig,
	QueueStore,
	Redactor,
	ResolverIO,
	RunStore,
	SessionLineageStore,
	SessionRegistry,
	TaskDefinition,
	TaskInstance,
	VerifyExecutor,
	WorkerDeps,
	WorktreeInfo,
} from "@queohoh/core";
import {
	buildLiveState,
	cronDue,
	finalizeRun,
	globalWorkspaceDir,
	instantiateDefinition,
	isProtectedWorktree,
	laneKey,
	listDefinitions,
	loadProjectDefaultBranch,
	loadProjectDefaultModels,
	loadProjectProtectedWorktrees,
	loadProjectTaskRetentionDays,
	loadProjectVars,
	parseCron,
	projectWorkspaceDir,
	qooTempName,
	REPO_SENTINEL,
	resolveDefinition,
	resolveTarget,
	schedule,
	startRun,
} from "@queohoh/core";
import { firstEnabledProvider } from "./settings-store.js";
import { inProcessSpawner, type ShimSpawner } from "./shim-host.js";

/**
 * Adoption verdict for a task that is `running` on disk but not managed by THIS
 * process (fresh boot, reload, or crash recovery). Pure so it can be unit-tested
 * away from the disk/pid machinery.
 * - `result.json` present → the shim finished while we were away → `finalize`.
 * - no result, but the recorded pid is alive AND its argv is a shim (guarding
 *   pid reuse — a recycled pid pointing at some unrelated process) → `adopt`.
 * - neither → the supervisor is gone → `orphan` (settle as `worker died`).
 */
export function adoptionDecision(
	hasResult: boolean,
	pidAlive: boolean,
	argvLooksLikeShim: boolean,
): "finalize" | "adopt" | "orphan" {
	if (hasResult) return "finalize";
	if (pidAlive && argvLooksLikeShim) return "adopt";
	return "orphan";
}

/**
 * Combine the local merged-back verdict with a PR's state into the `merged`
 * fact the TUI's `↣` marker reads. Pure so it can be unit-tested away from the
 * git/gh machinery.
 * - `localMerged` — HEAD-is-ancestor-of-default result (`true`/`false`/`null`
 *   unknown). A squash-merged branch reads `false` here (its commits never land
 *   on the default branch verbatim).
 * - `prState` — the matched PR's state (`"MERGED"`/`"OPEN"`/`null` no PR).
 *
 * `true` when EITHER signal says merged (this is what covers squash merges).
 * `null` only when BOTH are unknown; any other concrete signal reads `false`
 * (local `false`, or an `"OPEN"` PR — definitively not merged).
 */
export function foldMerged(
	localMerged: boolean | null,
	prState: string | null,
): boolean | null {
	if (localMerged === true || prState === "MERGED") return true;
	if (localMerged === null && prState === null) return null;
	return false;
}

/** Default liveness probe: `kill(pid, 0)` throws iff the pid is dead/unowned. */
function defaultPidAlive(pid: number): boolean {
	try {
		process.kill(pid, 0);
		return true;
	} catch {
		return false;
	}
}

/** Default shim-argv probe: `ps -p <pid> -o command=` prints the process's
 * command line; a live shim's argv contains `shim.js`. Any failure (ps missing,
 * pid gone between checks) → false, so a non-shim/dead pid never gets adopted. */
function defaultIsShimPid(pid: number): boolean {
	try {
		const out = execFileSync("ps", ["-p", String(pid), "-o", "command="], {
			encoding: "utf-8",
		});
		return out.includes("shim.js");
	} catch {
		return false;
	}
}

/** Per-worktree git/PR facts merged onto WorktreeInfo. Each field null = unknown. */
interface GitEnrichment {
	dirty: boolean | null;
	/** Worktree HEAD is an ancestor of the project's default branch (vars.yaml
	 * `default_branch`, fallback `main`) — its committed work has been merged
	 * back. null = unknown / not meaningful (the default-branch checkout
	 * itself, where "merged into yourself" would always be true). */
	merged: boolean | null;
	lastCommitEpoch: number | null;
	lastCommitAuthor: string | null;
	lastCommitAuthorEmail: string | null;
	lastCommitHash: string | null;
	/** Open PR number for this worktree's branch (via `gh pr list`). null =
	 * unknown / no open PR / gh unavailable. */
	prNumber: number | null;
	/** Web URL of that open PR (via `gh pr list`'s `url` field). null =
	 * unknown / no open PR / gh unavailable / gh omitted the field. Paired with
	 * prNumber so the TUI can open the PR on a click. */
	prUrl: string | null;
	/** Base branch of that PR (`gh`'s `baseRefName`, e.g. `main`). null when
	 * there is no PR / gh unavailable / field omitted. Goto feeds this to
	 * `juice --base <ref>` (TUI falls back to `origin/main` when null). */
	prBase: string | null;
	/** Display name of the PR's author — its `author.name`, falling back to
	 * `author.login`, null when both are empty / there is no PR. This is who
	 * OPENED the PR; for a squash-merged branch the local HEAD author is instead
	 * an automation merge commit, so the TUI prefers this in the Author column.
	 * Sourced from whichever of the two PR lists (open / merged) matched. */
	prAuthor: string | null;
	/** The matched PR's state: `"OPEN"` or `"MERGED"` (gh's `state` field). null
	 * when there is no PR / gh unavailable. Folded into `merged` below (a
	 * `"MERGED"` state supplements local ancestry, covering squash merges); kept
	 * on the wire as explicit supplementary detail. */
	prState: string | null;
	/** Whether the matched PR's review decision is APPROVED (gh's `reviewDecision`
	 * field === `"APPROVED"`). null when there is no PR / gh unavailable; false
	 * when a PR exists but is not approved (review required, changes requested, no
	 * reviewers). Drives the TUI's green approved marker, which shares the merged
	 * marker's front slot but yields to it (merged wins; see the TUI's
	 * `wt_merge_marker`). */
	approved: boolean | null;
	/** Whether the matched PR has the `ready-for-review` label. null when there
	 * is no PR / gh unavailable; false when a PR exists without the label. Shares
	 * the merge-marker front slot below approved (merge > approve > R > W). */
	readyForReview: boolean | null;
	/** Whether the matched PR has the `WIP` label. null when there is no PR /
	 * gh unavailable; false when a PR exists without the label. Lowest-priority
	 * front marker (merge > approve > ready-for-review > WIP). */
	wip: boolean | null;
}

/** The git-commit subset of GitEnrichment — everything computeGitEnrichment
 * derives from a single worktree path. The PR facts (prNumber/prUrl/prBase/
 * prAuthor/prState/approved/readyForReview/wip) are layered on separately:
 * they're per-repo facts, fetched once per sweep from `gh`, not per worktree. */
type GitCommitFacts = Omit<
	GitEnrichment,
	| "prNumber"
	| "prUrl"
	| "prBase"
	| "prAuthor"
	| "prState"
	| "approved"
	| "readyForReview"
	| "wip"
>;

/** Label names the TUI's front merge-marker slot cares about. Exact GitHub label
 * names (case-sensitive as gh returns them). */
const LABEL_READY_FOR_REVIEW = "ready-for-review";
const LABEL_WIP = "WIP";

/** One repo's PR facts, keyed by head branch: the number/url/base/state plus the
 * author's name and login (either may be empty on the wire → treated as null
 * when composing `prAuthor`). Populated from both the open and the recently
 * merged `gh pr list` calls (see ghPrMap). */
interface PrFacts {
	number: number;
	url: string | null;
	/** gh's `baseRefName` (e.g. `main`); null when omitted. */
	baseRef: string | null;
	state: string | null;
	authorName: string | null;
	authorLogin: string | null;
	/** gh's `reviewDecision` — `"APPROVED"`, `"CHANGES_REQUESTED"`,
	 * `"REVIEW_REQUIRED"`, or empty (no reviewers) → null. The caller reduces it
	 * to the `approved` boolean carried on the wire. */
	reviewDecision: string | null;
	/** gh's `labels[].name` values for this PR (empty when none / omitted). The
	 * caller reduces these to the `readyForReview` / `wip` wire booleans. */
	labelNames: string[];
}

/** Serve last-known enrichment for a worktree this long before re-shelling git.
 * 10s keeps the dirty marker near-live (user request; was 60s) — the sweep is
 * single-flighted and two cheap git commands per worktree, so the churn is
 * negligible at a dozen worktrees. */
const GIT_ENRICH_TTL_MS = 10_000;

/**
 * Terminal statuses — a task in one of these will never run again. Mirrors the
 * dismiss list in api.ts. Used by the worktree-deletion archive sweep.
 */
const TERMINAL_STATUSES: ReadonlySet<TaskInstance["status"]> = new Set([
	"done",
	"failed",
	"skipped",
	"cancelled",
	"verify-failed",
]);

/**
 * Name forms that all mean the same worktree directory: full `repo.branch`,
 * stripped display name, and the caller's request string. Used when cancelling
 * tasks that target a worktree being removed.
 */
export function worktreeNameAliases(
	repo: string,
	canonicalName: string,
	requestName: string,
): Set<string> {
	const names = new Set<string>([canonicalName, requestName]);
	const prefix = `${repo}.`;
	if (canonicalName.startsWith(prefix)) {
		names.add(canonicalName.slice(prefix.length));
	} else {
		names.add(`${prefix}${canonicalName}`);
	}
	if (requestName && requestName !== canonicalName) {
		if (requestName.startsWith(prefix)) {
			names.add(requestName.slice(prefix.length));
		} else {
			names.add(`${prefix}${requestName}`);
		}
	}
	return names;
}

/**
 * Whether a task is bound to one of the worktree names (resolved
 * `target.worktree`, or an unresolved `worktree:<name>` ref that would land
 * there). Does not use `laneKey` — a shared definition lane on another
 * worktree must not be cancelled when this worktree is removed.
 */
export function taskTargetsWorktreeNames(
	task: TaskInstance,
	repo: string,
	names: ReadonlySet<string>,
): boolean {
	if (task.target.repo !== repo) return false;
	const wt = task.target.worktree;
	if (wt !== null && wt !== REPO_SENTINEL && names.has(wt)) return true;
	// Unresolved yet: ref still points at a worktree name that is going away.
	if (wt === null) {
		const m = /^worktree:(.+)$/.exec(task.target.ref);
		if (m?.[1] && names.has(m[1])) return true;
	}
	return false;
}

export interface EngineDeps {
	store: QueueStore;
	runStore: RunStore;
	registry: SessionRegistry;
	config: GlobalConfig;
	resolverIO: ResolverIO;
	exec: Exec;
	executeClaude: ClaudeExecutor;
	executeVerify: VerifyExecutor;
	redact: Redactor;
	lineage: SessionLineageStore;
	onChange?: () => void;
	/** Wall-clock seam for cron evaluation; defaults to Date.now. Tests inject a
	 * controllable clock. */
	now?: () => number;
	/** Spawns a run's per-run process (production: `makeShimSpawner`, a detached
	 * `dist/shim.js` that owns the claude child). Absent → the Engine builds an
	 * in-process spawner from `executeClaude`, so existing callers/tests that only
	 * inject `executeClaude` keep their deterministic in-process behavior. */
	spawnShim?: ShimSpawner;
	/** Liveness probe for the adoption sweep; default `process.kill(pid, 0)`.
	 * Injected by tests to force a decision without a real process. */
	pidAlive?: (pid: number) => boolean;
	/** Argv probe distinguishing a live shim from a reused pid; default a
	 * `ps -p <pid> -o command=` check for `shim.js`. Injected by tests. */
	isShimPid?: (pid: number) => boolean;
	/** The provider the operator has currently switched to (persisted in the
	 * daemon's SettingsStore). Read fresh at each `buildWorkerDeps` so a mid-run
	 * `set_active_provider` re-heads the NEXT run's fallback chain. Absent ⇒ the
	 * precedence-first enabled provider from `config.providers` (used by tests
	 * that don't exercise the provider switch). */
	activeProvider?: () => string;
	/** Whether the operator has PAUSED the cron for the definition keyed
	 * `<repo>/<name>` (persisted in the daemon's SettingsStore). Read fresh each
	 * tick so a `set_cron_enabled` toggle takes effect on the next evaluation.
	 * Absent ⇒ nothing is paused (used by tests that don't exercise the toggle). */
	isCronDisabled?: (key: string) => boolean;
}

export class Engine {
	private running = new Map<string, Promise<void>>();
	// Spawned claude child pid per running task, populated via the worker's
	// onSpawned dep and cleared when the run settles. Absent for a task whose
	// worker never reported a pid (spawn failed) or that started under a previous
	// daemon process — stopTask throws in that case.
	private childPids = new Map<string, number>();
	// Task ids the user explicitly Stopped, so the worker settles their kill as
	// `cancelled` rather than `failed`. Populated by stopTask, cleared when the
	// run settles (startWorker's finally-then).
	private cancelledTaskIds = new Set<string>();
	// Adopt/finalize promises in flight from the adoption sweep, keyed by task id.
	// Guards a slow async finalize from being fired twice on consecutive ticks
	// (the sweep runs every tick); also awaited by `drain` so shutdown/tests wait
	// for an adopted finalize to settle, not just live workers.
	private finalizing = new Map<string, Promise<void>>();
	private ticking = false;
	// Cron fire-timing dedup: definition key ("repo/name") -> epoch ms of last
	// evaluation. In-memory by design — survives macOS sleep (process suspended,
	// not restarted); a true restart re-seeds to `now`, which is why nothing fires
	// on boot / hot-reload. See docs/superpowers/specs/2026-07-14-cron-scheduler-design.md.
	private cronCursor = new Map<string, number>();
	// Definitions whose async fire has not yet settled — guards a slow discovery
	// from being fired twice on consecutive ticks.
	private cronInFlight = new Set<string>();
	private worktreeCache = new Map<string, WorktreeInfo[]>(); // repo name -> worktrees
	// Repos whose `listWorktrees` has succeeded at least once this process. Guards
	// the worktree-deletion sweep against a seeded-empty cache (cold start or a
	// never-listable repo), where "worktree absent" would be a false positive.
	private worktreeListingOk = new Set<string>();
	// Git enrichment, keyed by worktree PATH, refreshed off the hot pass() path.
	private gitEnrichCache = new Map<string, GitEnrichment>();
	private gitEnrichFetchedAt = new Map<string, number>(); // path -> last fetch (ms)
	private enrichInFlight: Promise<void> | null = null; // single-flight guard (mirrors `ticking`)

	// Resolved once: the production spawner if injected, else an in-process
	// spawner built from executeClaude/redact so the daemon is agnostic of the
	// shim path and existing tests stay deterministic.
	private readonly spawnShim: ShimSpawner;

	constructor(private readonly deps: EngineDeps) {
		this.spawnShim =
			deps.spawnShim ?? inProcessSpawner(deps.executeClaude, deps.redact);
	}

	runningTaskIds(): string[] {
		return [...this.running.keys()];
	}

	/**
	 * Base worktree cache merged with any available git enrichment. Enrichment is
	 * populated asynchronously (see refreshGitEnrichment), so a worktree without a
	 * cache entry is returned unchanged (fields stay `undefined` → TUI unknown).
	 */
	worktreesByRepo(): Record<string, WorktreeInfo[]> {
		const out: Record<string, WorktreeInfo[]> = {};
		for (const [repo, list] of this.worktreeCache) {
			const repoPath = this.repoPath(repo);
			const protectedNames = loadProjectProtectedWorktrees(
				projectWorkspaceDir(this.deps.config, repo),
			);
			out[repo] = list.map((wt) => {
				const e = this.gitEnrichCache.get(wt.path);
				const base: WorktreeInfo = e ? { ...wt, ...e } : { ...wt };
				base.protected = isProtectedWorktree(
					repoPath,
					repo,
					protectedNames,
					wt,
				);
				return base;
			});
		}
		return out;
	}

	/** Await all in-flight workers AND adopted finalizes (test helper / shutdown). */
	async drain(): Promise<void> {
		await Promise.all([...this.running.values(), ...this.finalizing.values()]);
	}

	laneOfCwd(cwd: string): string | null {
		for (const [repo, worktrees] of this.worktreeCache) {
			for (const wt of worktrees) {
				if (cwd === wt.path || cwd.startsWith(`${wt.path}/`)) {
					return `${repo}:${wt.name}`;
				}
			}
		}
		return null;
	}

	/** Map an absolute path to the registered project + worktree containing it. */
	async resolveCwd(
		cwd: string,
	): Promise<{ repo: string; worktree: string } | null> {
		let best: { repo: string; worktree: string; path: string } | null = null;
		for (const project of this.deps.config.projects) {
			let list: WorktreeInfo[];
			try {
				list = await this.deps.resolverIO.listWorktrees(project.path);
			} catch {
				continue;
			}
			for (const wt of list) {
				if (cwd !== wt.path && !cwd.startsWith(`${wt.path}/`)) continue;
				if (best === null || wt.path.length > best.path.length) {
					best = { repo: project.name, worktree: wt.name, path: wt.path };
				}
			}
		}
		return best === null ? null : { repo: best.repo, worktree: best.worktree };
	}

	/** Best-effort git toplevel of a path, used for error guidance. */
	async gitToplevel(cwd: string): Promise<string | null> {
		try {
			const { stdout, exitCode } = await this.deps.exec(
				"git",
				["-C", cwd, "rev-parse", "--show-toplevel"],
				{ cwd },
			);
			const top = stdout.trim();
			return exitCode === 0 && top.length > 0 ? top : null;
		} catch {
			return null;
		}
	}

	/**
	 * Remove a worktree by name. `name` may be the full directory name
	 * (`<repo>.<branch>`) or the TUI's display name with the `<repo>.` prefix
	 * stripped — both are accepted because rows only carry the stripped form.
	 *
	 * Cancels live work on that worktree first (queued / needs-input immediately;
	 * running via stopTask, or force-cancelled if no child is tracked). Leaving
	 * those tasks alive after the dir is gone used to strand them as orphans
	 * that still looked "scheduled" while the worktree-deletion archive sweep
	 * only picked up already-terminal rows.
	 *
	 * The removal itself force-cleans the worktree, removes it via `wt`, then
	 * deletes the local branch (mirrors agent247's cleanup-worktree.sh) —
	 * this discards any uncommitted changes.
	 */
	async removeWorktree(repo: string, name: string): Promise<void> {
		const repoPath = this.repoPath(repo);
		if (repoPath === null) throw new Error(`unknown repo: ${repo}`);
		const list = await this.deps.resolverIO.listWorktrees(repoPath);
		const wt = list.find(
			(w) => w.name === name || w.name === `${repo}.${name}`,
		);
		if (!wt) throw new Error(`worktree not found: ${repo}:${name}`);
		const protectedNames = loadProjectProtectedWorktrees(
			projectWorkspaceDir(this.deps.config, repo),
		);
		if (isProtectedWorktree(repoPath, repo, protectedNames, wt)) {
			throw new Error(
				`Worktree "${wt.name}" is protected and cannot be removed`,
			);
		}
		this.cancelLiveTasksForWorktree(
			repo,
			worktreeNameAliases(repo, wt.name, name),
			"worktree removed",
		);
		await this.deps.resolverIO.removeWorktree(repoPath, wt);
		this.worktreeCache.delete(repo);
	}

	/**
	 * Cancel queued / needs-input / running tasks that target a worktree about
	 * to disappear (or already gone). Matches by `target.worktree` name aliases
	 * and by unresolved `worktree:<name>` refs — not by `laneKey`, so a
	 * definition-level lane override (e.g. testing1-stack) on another worktree
	 * is not cancelled when this one is removed.
	 */
	private cancelLiveTasksForWorktree(
		repo: string,
		names: ReadonlySet<string>,
		reason: string,
	): void {
		for (const t of this.deps.store.list()) {
			if (!taskTargetsWorktreeNames(t, repo, names)) continue;
			if (t.status === "running") {
				try {
					this.stopTask(t.id);
				} catch {
					// No tracked child (prior-daemon run, or spawn never reported) —
					// force terminal so the next archive sweep can dismiss it.
					this.deps.store.update(t.id, {
						status: "cancelled",
						error: reason,
						notBefore: null,
						startedAt: null,
					});
				}
			} else if (t.status === "queued" || t.status === "needs-input") {
				this.deps.store.update(t.id, {
					status: "cancelled",
					error: reason,
					notBefore: null,
				});
			}
		}
	}

	/**
	 * Create a worktree for `name` (the new branch) in `repo`. Rejects an unknown
	 * repo or a branch that already has a worktree; otherwise delegates to the
	 * resolver IO (`wt switch -c`) and invalidates the cache so the next snapshot
	 * lists it. No busy-guard — creation can't collide with a running task.
	 */
	/** Returns the created worktree's absolute path (the TUI opens a tmux
	 * window there after a create). */
	async createWorktree(repo: string, name: string): Promise<string> {
		const repoPath = this.repoPath(repo);
		if (repoPath === null) throw new Error(`unknown repo: ${repo}`);
		const list = await this.deps.resolverIO.listWorktrees(repoPath);
		if (list.some((w) => w.branch === name || w.name === `${repo}.${name}`)) {
			throw new Error(`worktree already exists: ${name}`);
		}
		const spawned = await this.deps.resolverIO.spawnWorktree(repoPath, name);
		this.worktreeCache.delete(repo);
		return spawned.path;
	}

	private repoPath(repo: string): string | null {
		return this.deps.config.projects.find((p) => p.name === repo)?.path ?? null;
	}

	/**
	 * Resolve a `(repo, worktree)` pair to its absolute checkout path — the same
	 * resolution the worker uses (incl. the `@repo` sentinel → primary checkout).
	 * Returns null for an unknown repo or worktree. Public so the `listSessions`
	 * RPC and the worker's `worktreePath` closure share one implementation.
	 */
	async worktreeAbsPath(
		repo: string,
		worktree: string,
	): Promise<string | null> {
		const path = this.repoPath(repo);
		if (!path) return null;
		// The `@repo` sentinel resolves to the project's primary checkout.
		if (worktree === REPO_SENTINEL) return path;
		const list = await this.deps.resolverIO.listWorktrees(path);
		return list.find((w) => w.name === worktree)?.path ?? null;
	}

	async tick(): Promise<void> {
		if (this.ticking) return;
		this.ticking = true;
		try {
			await this.pass();
		} catch (err) {
			console.error("engine tick error:", err);
		} finally {
			this.ticking = false;
		}
	}

	private async pass(): Promise<void> {
		const { deps } = this;
		deps.registry.sweep();
		this.evaluateCrons();
		await this.refreshWorktreeCache();
		// Fire-and-forget: git enrichment must never add latency to the pass. It
		// is TTL-throttled and single-flighted, and pushes onChange when it moves.
		void this.refreshGitEnrichment();

		// Adoption sweep: a task that is `running` on disk but not managed by THIS
		// process (fresh boot, reload, or crash recovery). result.json present → the
		// shim finished while we were away, finalize now; shim pid still alive (and
		// its argv is a shim, guarding pid reuse) → re-adopt, keep polling via the
		// tick; neither → the supervisor is gone, fail it.
		for (const t of deps.store.list()) {
			if (
				t.status !== "running" ||
				this.running.has(t.id) ||
				this.finalizing.has(t.id)
			) {
				continue;
			}
			const hasResult = deps.runStore.readResultJson(t.id) !== null;
			const pid = deps.runStore.readWorkerPid(t.id);
			const alive = pid !== null && this.isPidAlive(pid);
			const shimArgv = alive && this.isShimPidCheck(pid as number);
			const decision = adoptionDecision(hasResult, alive, shimArgv);
			if (decision === "finalize") {
				const deps2 = this.buildWorkerDeps(t);
				if (deps2) {
					const p = this.adoptAndFinalize(t.id, deps2).finally(() =>
						this.finalizing.delete(t.id),
					);
					this.finalizing.set(t.id, p);
				}
			} else if (decision === "adopt") {
				// Idempotent re-registration so Stop works and the lane stays busy.
				if (pid !== null) this.childPids.set(t.id, pid);
				const lane = laneKey(t) ?? t.id;
				deps.registry.registerWorker(t.id, lane, pid ?? process.pid);
			} else {
				deps.store.update(t.id, { status: "failed", error: "worker died" });
				this.childPids.delete(t.id);
				deps.registry.unregisterWorker(t.id);
			}
		}

		// Auto-archive old terminal tasks. `cancelled` is archived like `done`
		// because it's a deliberate, resolved outcome; `failed`/`skipped` are left
		// visible (they usually want attention or explain a stalled chain).
		// Retention is per-project: the project's `task_retention_days` (vars.yaml)
		// overrides the workspace `archive_after_days` default. Projects the store
		// knows but config/vars don't (removed projects, bad values) fall back to
		// that default. Cutoffs are memoized per repo for this sweep.
		const now = this.deps.now?.() ?? Date.now();
		const cutoffByRepo = new Map<string, number>();
		const cutoffFor = (repo: string): number => {
			const cached = cutoffByRepo.get(repo);
			if (cached !== undefined) return cached;
			const days = loadProjectTaskRetentionDays(
				projectWorkspaceDir(deps.config, repo),
				deps.config.archiveAfterDays,
			);
			const cutoff = now - days * 86_400_000;
			cutoffByRepo.set(repo, cutoff);
			return cutoff;
		};
		for (const t of deps.store.list()) {
			if (
				(t.status === "done" || t.status === "cancelled") &&
				Date.parse(t.created) < cutoffFor(t.target.repo)
			) {
				deps.store.archive(t.id);
			}
		}

		// Worktree gone: cancel live work first, then archive terminals.
		// Deleting a worktree (RPC or external) means "I'm done with this lane"
		// — queued/running/needs-input must not keep looking scheduled against a
		// path that no longer exists. Running stops via stopTask when we still
		// track a child; otherwise we force-cancel. The subsequent archive pass
		// picks up the now-terminal rows (and any that were already terminal).
		const worktreeStillPresent = (repo: string, wt: string): boolean => {
			if (!this.worktreeListingOk.has(repo)) return true; // cold cache: don't orphan
			const known = this.worktreeCache.get(repo) ?? [];
			return known.some((w) => w.name === wt);
		};
		for (const t of deps.store.list()) {
			const wt = t.target.worktree;
			if (wt === null || wt === REPO_SENTINEL) continue;
			if (worktreeStillPresent(t.target.repo, wt)) continue;
			const names = worktreeNameAliases(t.target.repo, wt, wt);
			// One cancel helper per orphaned task would re-scan the store; only
			// act on this task's status here (aliases still matter for the RPC
			// path where several name forms map to one dir).
			if (!taskTargetsWorktreeNames(t, t.target.repo, names)) continue;
			if (t.status === "running") {
				try {
					this.stopTask(t.id);
				} catch {
					deps.store.update(t.id, {
						status: "cancelled",
						error: "worktree removed",
						notBefore: null,
						startedAt: null,
					});
				}
			} else if (t.status === "queued" || t.status === "needs-input") {
				deps.store.update(t.id, {
					status: "cancelled",
					error: "worktree removed",
					notBefore: null,
				});
			}
		}
		for (const t of deps.store.list()) {
			const wt = t.target.worktree;
			if (
				!TERMINAL_STATUSES.has(t.status) ||
				wt === null ||
				wt === REPO_SENTINEL ||
				!this.worktreeListingOk.has(t.target.repo)
			) {
				continue;
			}
			const known = this.worktreeCache.get(t.target.repo) ?? [];
			if (!known.some((w) => w.name === wt)) {
				deps.store.archive(t.id);
			}
		}

		const tasks = deps.store.list();
		const live = buildLiveState(deps.registry.list(), tasks, (cwd) =>
			this.laneOfCwd(cwd),
		);
		const decision = schedule(tasks, live, {
			perProjectMax: deps.config.maxConcurrentTasks,
		});

		// Chain members whose predecessor did not succeed: mark terminal `skipped`
		// so they never run (stop-on-failure inside a chain). Not resource-limited.
		for (const { task, reason } of decision.skip) {
			deps.store.update(task.id, { status: "skipped", error: reason });
		}
		if (decision.skip.length > 0) deps.onChange?.();
		for (const task of decision.resolve) {
			await this.resolveTask(task);
		}
		for (const task of decision.start) {
			this.startWorker(task);
		}
	}

	/** Every definition with a non-null `cron`, across all projects. Global defs
	 * are shadowed by a project-local def of the same name (matches the API's
	 * `definitions` enumeration). A project whose tasks dir is unreadable is
	 * skipped, not fatal. */
	private cronDefinitions(): TaskDefinition[] {
		const out: TaskDefinition[] = [];
		for (const project of this.deps.config.projects) {
			try {
				const byName = new Map<string, TaskDefinition>();
				for (const def of listDefinitions(
					globalWorkspaceDir(this.deps.config),
					project.name,
				)) {
					byName.set(def.name, def);
				}
				for (const def of listDefinitions(
					projectWorkspaceDir(this.deps.config, project.name),
					project.name,
				)) {
					byName.set(def.name, def);
				}
				for (const def of byName.values()) {
					if (def.cron) out.push(def);
				}
			} catch {
				// Unreadable tasks dir: skip this project's crons for this tick.
			}
		}
		return out;
	}

	/** Fire any cron definition whose schedule has come due since its cursor.
	 * Synchronous and cheap when nothing is due (an in-memory `cronDue` check);
	 * the expensive discovery shell-out only runs on a due slot, and even then
	 * off the pass via fire-and-forget `fireCron`. */
	private evaluateCrons(): void {
		const now = this.deps.now?.() ?? Date.now();
		const defs = this.cronDefinitions();
		const liveKeys = new Set(defs.map((d) => `${d.repo}/${d.name}`));
		// Prune vanished defs so a re-added def re-seeds (no surprise catch-up).
		for (const key of [...this.cronCursor.keys()]) {
			if (!liveKeys.has(key)) this.cronCursor.delete(key);
		}
		for (const def of defs) {
			const key = `${def.repo}/${def.name}`;
			const cursor = this.cronCursor.get(key);
			if (cursor === undefined) {
				this.cronCursor.set(key, now); // first sight: seed, never fire on boot
				continue;
			}
			// Operator-paused cron: advance the cursor to `now` and skip firing, so
			// resuming it later never replays the slots missed while paused (same
			// catch-up-once guard as a parse error or first sight — a paused def is
			// "seen but silent", not "unseen and owed a backlog").
			if (this.deps.isCronDisabled?.(key)) {
				this.cronCursor.set(key, now);
				continue;
			}
			if (this.cronInFlight.has(key)) continue;
			let due: boolean;
			try {
				due = cronDue(parseCron(def.cron as string), cursor, now);
			} catch (err) {
				console.error(
					`cron parse error for ${key}: ${err instanceof Error ? err.message : String(err)}`,
				);
				this.cronCursor.set(key, now); // don't re-log every tick
				continue;
			}
			if (!due) continue;
			this.cronCursor.set(key, now); // advance BEFORE the async fire (no double-fire)
			this.cronInFlight.add(key);
			void this.fireCron(def).finally(() => this.cronInFlight.delete(key));
		}
	}

	/** Enqueue a cron fire through the same path as the runDefinition API: run
	 * discovery (if any) and create tasks with source "cron". Never throws — a
	 * failure is logged and the cursor stays advanced (no retry-spam). */
	private async fireCron(def: TaskDefinition): Promise<void> {
		const { deps } = this;
		const project = deps.config.projects.find((p) => p.name === def.repo);
		if (!project) return;
		const projectDir = projectWorkspaceDir(deps.config, def.repo);
		try {
			const repoVars = loadProjectVars(projectDir);
			const created = await instantiateDefinition(
				def,
				def.discovery ? { mode: "discover" } : { mode: "args", values: [] },
				{
					store: deps.store,
					exec: deps.exec,
					cwd: projectDir,
					source: "cron",
					globalVars: {
						project: def.repo,
						repo_path: project.path,
						...deps.config.vars,
					},
					repoVars,
				},
			);
			if (created.length > 0) deps.onChange?.();
		} catch (err) {
			console.error(
				`cron fire failed for ${def.repo}/${def.name}: ${err instanceof Error ? err.message : String(err)}`,
			);
		}
	}

	private async refreshWorktreeCache(): Promise<void> {
		for (const project of this.deps.config.projects) {
			try {
				this.worktreeCache.set(
					project.name,
					await this.deps.resolverIO.listWorktrees(project.path),
				);
				this.worktreeListingOk.add(project.name);
			} catch {
				// Transient git failure (e.g. index.lock contention): KEEP the
				// last-known list instead of clobbering it with [] — the clobber
				// made every visible row of the repo vanish for a tick, which the
				// user saw as flashing. A repo with no prior entry still records
				// [] so downstream lookups see it as known-empty.
				if (!this.worktreeCache.has(project.name)) {
					this.worktreeCache.set(project.name, []);
				}
			}
		}
	}

	/**
	 * Refresh per-worktree git facts (dirty/lastCommit epoch+author) off the hot
	 * path. Single-flighted via `enrichInFlight`, TTL-throttled per worktree, prunes
	 * dead paths, and fires `onChange` only when a value actually moved. Public so
	 * tests can await it deterministically; in production it is fire-and-forget.
	 * Never throws — every helper swallows errors into null.
	 */
	refreshGitEnrichment(): Promise<void> {
		// Single-flight: while a run is active, hand back the same promise rather
		// than starting a second concurrent sweep. tick()'s fire-and-forget kick
		// and a test's explicit await therefore share one deterministic run.
		if (this.enrichInFlight) return this.enrichInFlight;
		this.enrichInFlight = this.runGitEnrichment().finally(() => {
			this.enrichInFlight = null;
		});
		return this.enrichInFlight;
	}

	private async runGitEnrichment(): Promise<void> {
		const now = Date.now();
		let changed = false;
		const live = new Set<string>();
		for (const [repo, list] of this.worktreeCache) {
			// Branch→PR-facts map for this repo, fetched at most ONCE per sweep
			// (two gh calls — open + merged) and only when at least one worktree
			// here is actually being refreshed (all within TTL → no gh call at
			// all). Lazy so a repo served entirely from cache costs nothing.
			// `undefined` = not yet fetched this sweep.
			let prMap: Map<string, PrFacts> | null | undefined;
			const repoPath = this.repoPath(repo);
			// The branch the merged-back marker compares against — one vars.yaml
			// read per repo per sweep (same cadence as the protected-names read
			// in worktreesByRepo; a file read is noise next to the git calls).
			const defaultBranch = loadProjectDefaultBranch(
				projectWorkspaceDir(this.deps.config, repo),
			);
			for (const wt of list) {
				live.add(wt.path);
				if (
					now - (this.gitEnrichFetchedAt.get(wt.path) ?? 0) <
					GIT_ENRICH_TTL_MS
				)
					continue; // serve last-known within TTL
				if (prMap === undefined) {
					prMap = repoPath === null ? null : await this.ghPrMap(repoPath);
				}
				const facts = await this.computeGitEnrichment(
					wt.path,
					wt.branch,
					defaultBranch,
				);
				const pr = prMap?.get(wt.branch) ?? null;
				const prNumber = pr?.number ?? null;
				const prUrl = pr?.url ?? null;
				const prBase = pr?.baseRef ?? null;
				const prState = pr?.state ?? null;
				// PR author display: name preferred, else login, else null.
				const prAuthor = pr ? (pr.authorName ?? pr.authorLogin ?? null) : null;
				// Fold the PR state into the local ancestry verdict: a squash-merged
				// branch reads NOT an ancestor of the default branch (its commits
				// aren't on it), so local `merged` is false there — but a `"MERGED"`
				// PR state proves it landed. `merged` stays null only when BOTH
				// signals are unknown; any concrete signal that isn't "merged"
				// (local false, or an OPEN PR) reads as not merged.
				const merged = foldMerged(facts.merged, prState);
				// Approved is a per-PR fact (gh's reviewDecision). null when there is
				// no PR (unknown); a PR that isn't approved reads false. It shares the
				// merged marker's slot in the TUI but yields to merged.
				const approved = pr ? pr.reviewDecision === "APPROVED" : null;
				// Label markers: null with no PR; false when the PR exists without
				// the label. Same front slot as merge/approve; precedence is
				// merge > approve > ready-for-review > WIP (see TUI wt_merge_marker).
				const readyForReview = pr
					? pr.labelNames.includes(LABEL_READY_FOR_REVIEW)
					: null;
				const wip = pr ? pr.labelNames.includes(LABEL_WIP) : null;
				const e: GitEnrichment = {
					...facts,
					merged,
					prNumber,
					prUrl,
					prBase,
					prAuthor,
					prState,
					approved,
					readyForReview,
					wip,
				};
				this.gitEnrichFetchedAt.set(wt.path, Date.now());
				const prev = this.gitEnrichCache.get(wt.path);
				if (
					!prev ||
					prev.dirty !== e.dirty ||
					prev.merged !== e.merged ||
					prev.lastCommitEpoch !== e.lastCommitEpoch ||
					prev.lastCommitAuthor !== e.lastCommitAuthor ||
					prev.lastCommitAuthorEmail !== e.lastCommitAuthorEmail ||
					prev.lastCommitHash !== e.lastCommitHash ||
					prev.prNumber !== e.prNumber ||
					prev.prUrl !== e.prUrl ||
					prev.prBase !== e.prBase ||
					prev.prAuthor !== e.prAuthor ||
					prev.prState !== e.prState ||
					prev.approved !== e.approved ||
					prev.readyForReview !== e.readyForReview ||
					prev.wip !== e.wip
				) {
					changed = true;
				}
				this.gitEnrichCache.set(wt.path, e);
			}
		}
		// Prune worktrees that no longer exist.
		for (const path of [...this.gitEnrichCache.keys()]) {
			if (!live.has(path)) {
				this.gitEnrichCache.delete(path);
				this.gitEnrichFetchedAt.delete(path);
			}
		}
		if (changed) this.deps.onChange?.();
	}

	private async computeGitEnrichment(
		path: string,
		branch: string,
		defaultBranch: string,
	): Promise<GitCommitFacts> {
		const dirty = await this.gitDirty(path);
		const merged = await this.gitMerged(path, branch, defaultBranch);
		const {
			epoch: lastCommitEpoch,
			author: lastCommitAuthor,
			email: lastCommitAuthorEmail,
			hash: lastCommitHash,
		} = await this.gitLastCommit(path);
		return {
			dirty,
			merged,
			lastCommitEpoch,
			lastCommitAuthor,
			lastCommitAuthorEmail,
			lastCommitHash,
		};
	}

	/** Whether the worktree's HEAD is an ancestor of the project's default
	 * branch — i.e. its committed work has been merged back. null on the
	 * default-branch checkout itself (trivially its own ancestor — the marker
	 * would be permanent noise there), on a missing default branch, or on any
	 * git failure. NOTE: this is only an ancestry check — a squash-merged branch
	 * reads as NOT merged here, because its commits genuinely aren't on the
	 * default branch. `runGitEnrichment` supplements this verdict with the PR's
	 * state (a `"MERGED"` PR folds `merged` true; see `foldMerged`), which is
	 * what covers the squash-merge case. */
	private async gitMerged(
		path: string,
		branch: string,
		defaultBranch: string,
	): Promise<boolean | null> {
		if (branch.length === 0 || branch === defaultBranch) return null;
		try {
			const { exitCode } = await this.deps.exec(
				"git",
				["-C", path, "merge-base", "--is-ancestor", "HEAD", defaultBranch],
				{ cwd: path },
			);
			// merge-base exits 0 = ancestor, 1 = not an ancestor; anything else
			// (128: unknown ref, not a repo) is a real failure → unknown.
			if (exitCode === 0) return true;
			if (exitCode === 1) return false;
			return null;
		} catch {
			return null;
		}
	}

	/** True when the working tree has uncommitted changes; null on failure. */
	private async gitDirty(path: string): Promise<boolean | null> {
		try {
			const { stdout, exitCode } = await this.deps.exec(
				"git",
				["-C", path, "status", "--porcelain", "--untracked-files=normal"],
				{ cwd: path },
			);
			if (exitCode !== 0) return null;
			return stdout.trim().length > 0;
		} catch {
			return null;
		}
	}

	/**
	 * HEAD's commit epoch SECONDS + author name + author email + short hash in
	 * ONE `git log` call: `--format=%ct%x09%an%x09%ae%x09%h` prints
	 * "<epoch>\t<author>\t<email>\t<hash>". Fields map positionally; any that is
	 * absent (a shorter line, e.g. the old 3-field format before %h was appended)
	 * or empty/unparseable yields null. Positional parse assumes the author name
	 * carries no tab (git author names don't in practice) — the trade for being
	 * able to append trailing fields without ambiguity.
	 */
	private async gitLastCommit(path: string): Promise<{
		epoch: number | null;
		author: string | null;
		email: string | null;
		hash: string | null;
	}> {
		const none = { epoch: null, author: null, email: null, hash: null };
		try {
			const { stdout, exitCode } = await this.deps.exec(
				"git",
				["-C", path, "log", "-1", "--format=%ct%x09%an%x09%ae%x09%h"],
				{ cwd: path },
			);
			if (exitCode !== 0) return none;
			const parts = stdout.trim().split("\t");
			const n = Number.parseInt(parts[0] ?? "", 10);
			const author = (parts[1] ?? "").trim();
			const email = (parts[2] ?? "").trim();
			const hash = (parts[3] ?? "").trim();
			return {
				epoch: Number.isFinite(n) ? n : null,
				author: author.length > 0 ? author : null,
				email: email.length > 0 ? email : null,
				hash: hash.length > 0 ? hash : null,
			};
		} catch {
			return none;
		}
	}

	/**
	 * PRs for a repo as a branch→facts map, via TWO `gh pr list` calls at the
	 * repo root: the OPEN PRs, and the recently MERGED ones (limit 100). Two
	 * calls rather than one `--state all` so an old open PR beyond a merged
	 * window is never lost, and — because merges keep the daemon's PR knowledge
	 * alive after a PR drops out of the open list — a squash-merged branch still
	 * carries its true state + author (the fix for the wrong-author / no-`↣`
	 * bug). On a branch-name collision (a reused branch with a fresh open PR) the
	 * OPEN PR wins: it is overlaid onto the merged base LAST.
	 *
	 * Each call fails independently — one call's failure (gh missing,
	 * unauthenticated, non-zero exit, unparseable JSON) never discards the
	 * other's rows; only when BOTH yield no data does the whole map return null.
	 */
	private async ghPrMap(
		repoPath: string,
	): Promise<Map<string, PrFacts> | null> {
		const [open, merged] = await Promise.all([
			this.ghPrList(repoPath, "open", 200),
			this.ghPrList(repoPath, "merged", 100),
		]);
		if (open === null && merged === null) return null;
		const map = new Map<string, PrFacts>();
		// Merged first as the base, then open overlaid so the OPEN PR wins a
		// branch-name collision (an entry present in both lists).
		for (const [branch, fact] of merged ?? []) map.set(branch, fact);
		for (const [branch, fact] of open ?? []) map.set(branch, fact);
		return map;
	}

	/**
	 * One `gh pr list --state <state>` call as a branch→facts map, or null on any
	 * failure (gh missing, unauthenticated, non-zero exit, unparseable JSON) —
	 * never throws. A row with a non-string `url` keeps its number but stamps url
	 * null (gh always sends it; this only guards a malformed/forward-compat
	 * payload). `author` is `{name, login}` — either may be empty; both are
	 * carried so the caller composes `prAuthor` as name-else-login. Logged at
	 * most once per call at debug so a gh-less machine doesn't spam.
	 */
	private async ghPrList(
		repoPath: string,
		state: "open" | "merged",
		limit: number,
	): Promise<Map<string, PrFacts> | null> {
		try {
			const { stdout, exitCode } = await this.deps.exec(
				"gh",
				[
					"pr",
					"list",
					"--state",
					state,
					"--json",
					"number,headRefName,baseRefName,url,state,author,reviewDecision,labels",
					"--limit",
					String(limit),
				],
				{ cwd: repoPath },
			);
			if (exitCode !== 0) {
				console.debug?.(
					`gh pr list --state ${state}: non-zero exit ${exitCode}; skipping PR enrichment`,
				);
				return null;
			}
			const rows: unknown = JSON.parse(stdout);
			if (!Array.isArray(rows)) return null;
			const map = new Map<string, PrFacts>();
			for (const row of rows) {
				if (row === null || typeof row !== "object") continue;
				const {
					headRefName,
					number,
					url,
					baseRefName,
					state: prState,
					author,
					reviewDecision,
					labels,
				} = row as {
					headRefName?: unknown;
					number?: unknown;
					url?: unknown;
					baseRefName?: unknown;
					state?: unknown;
					author?: unknown;
					reviewDecision?: unknown;
					labels?: unknown;
				};
				if (typeof headRefName !== "string" || typeof number !== "number") {
					continue;
				}
				// author is `{name, login}`; an empty string reads as null so the
				// caller can fall back name → login → null.
				const a = (author ?? {}) as { name?: unknown; login?: unknown };
				const authorName =
					typeof a.name === "string" && a.name.length > 0 ? a.name : null;
				const authorLogin =
					typeof a.login === "string" && a.login.length > 0 ? a.login : null;
				// labels is `[{name, ...}, ...]`; keep only non-empty name strings.
				// Omitted / malformed → empty list (caller's includes() reads false).
				const labelNames: string[] = [];
				if (Array.isArray(labels)) {
					for (const lab of labels) {
						if (lab === null || typeof lab !== "object") continue;
						const name = (lab as { name?: unknown }).name;
						if (typeof name === "string" && name.length > 0) {
							labelNames.push(name);
						}
					}
				}
				map.set(headRefName, {
					number,
					url: typeof url === "string" ? url : null,
					baseRef:
						typeof baseRefName === "string" && baseRefName.length > 0
							? baseRefName
							: null,
					state: typeof prState === "string" ? prState : null,
					authorName,
					authorLogin,
					// gh sends "" for a PR with no reviewers; treat that (and any
					// non-string) as null so the caller's `=== "APPROVED"` reads false.
					reviewDecision:
						typeof reviewDecision === "string" && reviewDecision.length > 0
							? reviewDecision
							: null,
					labelNames,
				});
			}
			return map;
		} catch {
			console.debug?.(
				`gh pr list --state ${state}: unavailable; skipping PR enrichment`,
			);
			return null;
		}
	}

	private async resolveTask(task: TaskInstance): Promise<void> {
		const { deps } = this;
		const repoPath = this.repoPath(task.target.repo);
		if (repoPath === null) {
			deps.store.update(task.id, {
				status: "needs-input",
				error: `unknown repo: ${task.target.repo}`,
			});
			deps.onChange?.();
			return;
		}
		try {
			// Temp names slug from the task's run-specific content: a def task's
			// `prompt` is the rendered TEMPLATE (identical opening words for every
			// run), so its itemKey — the rendered args — names the worktree/branch;
			// an ad-hoc task's prompt IS the content.
			const resolution = await resolveTarget(
				task.target.ref,
				{ repoPath, tempName: () => qooTempName(task.itemKey ?? task.prompt) },
				deps.resolverIO,
			);
			if (resolution.outcome === "resolved") {
				deps.store.update(task.id, {
					target: { ...task.target, worktree: resolution.worktree },
					ephemeralWorktree: resolution.ephemeral,
				});
				this.worktreeCache.delete(task.target.repo); // stale after spawn
				// A chain resolves its worktree ONCE, at the head: stamp it onto the
				// tail members so they land on the same lane and never re-resolve
				// (which for a `temp` chain would spawn N worktrees).
				this.stampChainWorktree(task, resolution.worktree);
			} else {
				deps.store.update(task.id, {
					status: "needs-input",
					error: resolution.reason,
				});
			}
		} catch (err) {
			deps.store.update(task.id, {
				status: "failed",
				error: err instanceof Error ? err.message : String(err),
			});
		}
		deps.onChange?.();
	}

	/**
	 * When a chain HEAD resolves, stamp its resolved worktree onto every other
	 * member of the chain (they all share the one lane): pin the ref to
	 * `worktree:<name>` and clear the ephemeral flag (the head owns any temp
	 * worktree's lifecycle, not the tail). No-op for a non-head or a standalone
	 * task. Idempotent — only stamps members still unresolved.
	 */
	private stampChainWorktree(head: TaskInstance, worktree: string): void {
		if (head.chainId == null || head.chainSeq !== 0) return;
		for (const t of this.deps.store.list()) {
			if (
				t.chainId === head.chainId &&
				t.id !== head.id &&
				t.target.worktree === null
			) {
				this.deps.store.update(t.id, {
					target: { ...t.target, ref: `worktree:${worktree}`, worktree },
					ephemeralWorktree: false,
				});
			}
		}
	}

	/**
	 * Build the per-task WorkerDeps shared by a live run (`runLive`) and an adopted
	 * finalize (`adoptAndFinalize`). Returns null AFTER having already marked the
	 * task failed + fired onChange on a malformed vars.yaml — the sole pre-spawn
	 * failure that must fail the task rather than wedge startup. `onSpawned` and the
	 * shim spawn are NOT here (they are per-call, only for the live path); the
	 * `isCancelled` closure reads both the in-memory Stop set AND the persisted
	 * cancel marker, so a Stop that raced a daemon death still settles `cancelled`.
	 */
	private buildWorkerDeps(task: TaskInstance): WorkerDeps | null {
		const { deps } = this;

		// Load project vars before registering the worker; a malformed vars.yaml
		// must fail the task rather than wedge worker startup.
		let repoVars: Record<string, string>;
		try {
			repoVars = loadProjectVars(
				projectWorkspaceDir(deps.config, task.target.repo),
			);
		} catch (err) {
			deps.store.update(task.id, {
				status: "failed",
				error: err instanceof Error ? err.message : String(err),
			});
			deps.onChange?.();
			return null;
		}

		// Effective model catalog for this task's repo (BUILTIN_CATALOG ⊕
		// config.yaml `catalog:`), already merged at `loadGlobalConfig` — every
		// `provider/label` ref a task/definition names resolves against it.
		const catalog = deps.config.catalog;
		// Ordered fallback model-ref list a task with NO `model:` of its own
		// resolves against. A project's vars.yaml `default_models:` overrides the
		// global list — but only when it is a NON-EMPTY list: `loadProjectDefaultModels`
		// returns `[]` for an explicitly-empty / all-invalid list, and the ENGINE
		// owns the []→global fallback decision (an empty override means "unset").
		const projectDefaults = loadProjectDefaultModels(
			projectWorkspaceDir(deps.config, task.target.repo),
		);
		const defaultModels =
			projectDefaults && projectDefaults.length > 0
				? projectDefaults
				: deps.config.defaultModels;

		const repoPath = this.repoPath(task.target.repo);
		// Effective provider table: `deps.config.providers` is already built-in ⊕
		// config.yaml `providers:` (computed once at `loadGlobalConfig`). Per-provider
		// model tiers and the per-project `providers:` override are gone (catalog.ts
		// replaced them), so this is now forwarded as-is.
		const providers = deps.config.providers;
		// Which provider the operator has switched to (SettingsStore), read fresh
		// so a mid-run switch re-heads the next run. Absent seam ⇒ the
		// precedence-first enabled provider from the effective table.
		const activeProvider =
			this.deps.activeProvider?.() ?? firstEnabledProvider(providers);
		return {
			store: deps.store,
			runStore: deps.runStore,
			exec: deps.exec,
			executeClaude: deps.executeClaude,
			executeVerify: deps.executeVerify,
			redact: deps.redact,
			lineage: deps.lineage,
			// Builtin vars sit below explicit config vars (which spread last and can
			// override them); hooks rendered by the worker see them too.
			globalVars: {
				project: task.target.repo,
				repo_path: repoPath ?? "",
				...deps.config.vars,
			},
			repoVars,
			// A Stop settles as `cancelled` (not `failed`). Read BOTH the in-memory
			// set (live Stop) and the persisted marker (a Stop that raced a daemon
			// death, replayed on adoption).
			isCancelled: (id) =>
				this.cancelledTaskIds.has(id) || deps.runStore.readCancelMarker(id),
			catalog,
			defaultModels,
			providers,
			activeProvider,
			loadDef: (definition) => {
				const [repo, ...nameParts] = definition.split("/");
				const name = nameParts.join("/");
				if (!repo || !this.repoPath(repo)) return null;
				try {
					// Project-local defs first, then the global tasks dir.
					return resolveDefinition(this.deps.config, repo, name);
				} catch {
					return null;
				}
			},
			worktreePath: (repo, worktree) => this.worktreeAbsPath(repo, worktree),
			// 3h wall-clock ceiling. Idle reaping (12m, see runner.ts IDLE_TIMEOUT_MS)
			// catches wedged workers early, so this ceiling is a generous backstop —
			// not the primary reaper — for a run that keeps streaming but never
			// actually finishes. The default MODEL leg is gone (see WorkerDeps): a
			// model-less task resolves against `defaultModels`.
			defaults: { timeoutMs: 10_800_000 },
		};
	}

	private startWorker(task: TaskInstance): void {
		const deps = this.buildWorkerDeps(task);
		if (deps === null) return; // already marked failed + onChange fired
		const lane = laneKey(task) ?? task.id;
		this.deps.registry.registerWorker(task.id, lane, process.pid);
		const promise = this.runLive(task.id, deps)
			.catch((err) => {
				try {
					this.deps.store.update(task.id, {
						status: "failed",
						error: err instanceof Error ? err.message : String(err),
					});
				} catch {}
			})
			.then(() => this.cleanupRun(task.id));
		this.running.set(task.id, promise);
		this.deps.onChange?.();
	}

	/** Drive one run to settlement: pre-spawn prep (`startRun`), then the shim
	 * spawn. A null shim result means the supervisor died with no result.json →
	 * settle as `worker died`; otherwise finalize with the shim's result. The shim
	 * pid (production) or the claude child pid (in-process) is tracked for Stop and
	 * persisted so a returning daemon's adoption sweep can find it. */
	private async runLive(taskId: string, deps: WorkerDeps): Promise<void> {
		const start = await startRun(taskId, deps);
		if (start.kind === "settled") return; // failed pre-spawn; nothing spawned
		const result = await this.spawnShim(taskId, start.spec, (pid) => {
			this.childPids.set(taskId, pid);
			deps.runStore.writeWorkerPid(taskId, pid); // shim pid (production)
		});
		if (result === null) {
			await this.settleWorkerDied(taskId, deps);
			return;
		}
		const outcome = await finalizeRun(taskId, result, deps);
		// `outcome.retry` means an availability failure (session limit / out of
		// budget / provider unavailable) with a next provider left in the chain:
		// `finalizeRun` already stamped the task back to `queued` (with this
		// attempt's provider appended to `attemptedModels`) instead of
		// settling it terminal — there is nothing left to do here. The caller's
		// `.then(() => this.cleanupRun(...))` still runs regardless (frees the
		// lane/pid tracking), so the next `pass()` sees a `queued` task with a
		// free lane and schedules it straight back onto `startWorker`, whose
		// fresh `startRun` → `resolveRunContext` resolves onto the chain's next
		// candidate (this attempt's provider is now filtered out). A non-retry
		// outcome needs no extra handling either — the task already settled
		// terminal inside `finalizeRun`, exactly as before this task's changes.
		if (outcome.retry) return;
	}

	private cleanupRun(taskId: string): void {
		this.running.delete(taskId);
		this.childPids.delete(taskId);
		this.cancelledTaskIds.delete(taskId);
		this.deps.registry.unregisterWorker(taskId);
		this.deps.onChange?.();
	}

	/** No result.json and the shim is gone: settle as worker died (a report is
	 * still written so the detail pane isn't blank). Mirrors the sweep's orphan. */
	private async settleWorkerDied(
		taskId: string,
		deps: WorkerDeps,
	): Promise<void> {
		deps.runStore.finishRun(
			taskId,
			{
				result: {
					exitCode: 1,
					timedOut: false,
					signal: null,
					sessionId: null,
					resultText: "",
					stderr: "worker died",
					usage: {
						costUsd: null,
						turns: null,
						durationMs: null,
						inputTokens: null,
						outputTokens: null,
					},
				},
				outcome: "failed",
				reason: "worker died",
			},
			deps.redact,
		);
		deps.store.update(taskId, { status: "failed", error: "worker died" });
	}

	/** Adopt a run the sweep found `finalize`-able: read its result.json and
	 * finalize (or settle `worker died` if the result vanished between the sweep's
	 * probe and here), then release its lane/pid tracking. */
	private async adoptAndFinalize(
		taskId: string,
		deps: WorkerDeps,
	): Promise<void> {
		const result = deps.runStore.readResultJson(taskId);
		if (result === null) {
			await this.settleWorkerDied(taskId, deps);
		} else {
			// Same `{ retry }` handling as `runLive` — an adopted run (finalized
			// after a daemon restart) can retry-signal exactly like a live one, and
			// the lane/pid release below applies either way, so there is nothing
			// conditional to do with the outcome itself; see the comment in
			// `runLive` for the full rationale.
			await finalizeRun(taskId, result, deps);
		}
		this.childPids.delete(taskId);
		this.cancelledTaskIds.delete(taskId);
		this.deps.registry.unregisterWorker(taskId);
		this.deps.onChange?.();
	}

	/** Liveness probe honoring an injected `pidAlive`; default `kill(pid, 0)`. */
	private isPidAlive(pid: number): boolean {
		return (this.deps.pidAlive ?? defaultPidAlive)(pid);
	}

	/** Shim-argv probe honoring an injected `isShimPid`; default a `ps` check. */
	private isShimPidCheck(pid: number): boolean {
		return (this.deps.isShimPid ?? defaultIsShimPid)(pid);
	}

	/**
	 * Stop a running task by killing its claude child's process group, mirroring
	 * runner's timeout path: SIGTERM the group (fallback to a direct kill), then
	 * an unref'd 5s SIGKILL escalation. Records the id as user-cancelled first, so
	 * the worker settles the resulting kill signal as `cancelled` (not `failed`).
	 * Throws when no pid is tracked for the id — the task started under a previous
	 * daemon process, or its spawn never reported.
	 */
	stopTask(taskId: string): void {
		const pid = this.childPids.get(taskId);
		if (pid === undefined) {
			throw new Error(`no running child tracked for task: ${taskId}`);
		}
		this.cancelledTaskIds.add(taskId);
		// Persist the Stop BEFORE signalling: if the daemon dies between here and
		// the run settling, the adoption sweep replays the marker and settles the
		// run as `cancelled` rather than `failed` / `worker died`.
		this.deps.runStore.writeCancelMarker(taskId);
		try {
			process.kill(-pid, "SIGTERM");
		} catch {
			try {
				process.kill(pid, "SIGTERM");
			} catch {}
		}
		const killTimer = setTimeout(() => {
			try {
				process.kill(-pid, "SIGKILL");
			} catch {}
		}, 5000);
		killTimer.unref();
	}
}
