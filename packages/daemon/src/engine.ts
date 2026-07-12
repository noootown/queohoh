import type {
	ClaudeExecutor,
	Exec,
	GlobalConfig,
	MainSessionStore,
	QueueStore,
	Redactor,
	ResolverIO,
	RunStore,
	SessionRegistry,
	TaskInstance,
	VerifyExecutor,
	WorktreeInfo,
} from "@queohoh/core";
import {
	buildLiveState,
	effectiveModelTable,
	laneKey,
	loadProjectModels,
	loadProjectVars,
	projectWorkspaceDir,
	qooTempName,
	REPO_SENTINEL,
	resolveDefinition,
	resolveTarget,
	runTask,
	schedule,
} from "@queohoh/core";

/** Per-worktree git/PR facts merged onto WorktreeInfo. Each field null = unknown. */
interface GitEnrichment {
	dirty: boolean | null;
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
}

/** The git-commit subset of GitEnrichment — everything computeGitEnrichment
 * derives from a single worktree path. prNumber/prUrl are layered on separately
 * (they're per-repo facts, fetched once per sweep, not per worktree). */
type GitCommitFacts = Omit<GitEnrichment, "prNumber" | "prUrl">;

/** Serve last-known enrichment for a worktree this long before re-shelling git.
 * 10s keeps the dirty marker near-live (user request; was 60s) — the sweep is
 * single-flighted and two cheap git commands per worktree, so the churn is
 * negligible at a dozen worktrees. */
const GIT_ENRICH_TTL_MS = 10_000;

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
	mainSessions: MainSessionStore;
	onChange?: () => void;
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
	private ticking = false;
	private worktreeCache = new Map<string, WorktreeInfo[]>(); // repo name -> worktrees
	// Git enrichment, keyed by worktree PATH, refreshed off the hot pass() path.
	private gitEnrichCache = new Map<string, GitEnrichment>();
	private gitEnrichFetchedAt = new Map<string, number>(); // path -> last fetch (ms)
	private enrichInFlight: Promise<void> | null = null; // single-flight guard (mirrors `ticking`)

	constructor(private readonly deps: EngineDeps) {}

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
			out[repo] = list.map((wt) => {
				const e = this.gitEnrichCache.get(wt.path);
				return e ? { ...wt, ...e } : wt;
			});
		}
		return out;
	}

	/** Await all in-flight workers (test helper / shutdown). */
	async drain(): Promise<void> {
		await Promise.all([...this.running.values()]);
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
	 * Refuses while a task is running on the worktree's lane. The removal itself
	 * force-cleans the worktree, removes it via `wt`, then deletes the local
	 * branch (mirrors agent247's cleanup-worktree.sh) — this discards any
	 * uncommitted changes.
	 */
	async removeWorktree(repo: string, name: string): Promise<void> {
		const repoPath = this.repoPath(repo);
		if (repoPath === null) throw new Error(`unknown repo: ${repo}`);
		const list = await this.deps.resolverIO.listWorktrees(repoPath);
		const wt = list.find(
			(w) => w.name === name || w.name === `${repo}.${name}`,
		);
		if (!wt) throw new Error(`worktree not found: ${repo}:${name}`);
		const lanes = new Set([`${repo}:${wt.name}`, `${repo}:${name}`]);
		const busy = this.deps.store
			.list()
			.some((t) => t.status === "running" && lanes.has(laneKey(t) ?? ""));
		if (busy) throw new Error(`worktree busy: a task is running on ${wt.name}`);
		await this.deps.resolverIO.removeWorktree(repoPath, wt);
		this.worktreeCache.delete(repo);
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
		await this.refreshWorktreeCache();
		// Fire-and-forget: git enrichment must never add latency to the pass. It
		// is TTL-throttled and single-flighted, and pushes onChange when it moves.
		void this.refreshGitEnrichment();

		// Orphan sweep: running on disk but not in this process.
		for (const t of deps.store.list()) {
			if (t.status === "running" && !this.running.has(t.id)) {
				deps.store.update(t.id, {
					status: "failed",
					error: "orphaned by daemon restart",
				});
			}
		}

		// Auto-archive old terminal tasks. `cancelled` is archived like `done`
		// because it's a deliberate, resolved outcome; `failed`/`skipped` are left
		// visible (they usually want attention or explain a stalled chain).
		const cutoff = Date.now() - deps.config.archiveAfterDays * 86_400_000;
		for (const t of deps.store.list()) {
			if (
				(t.status === "done" || t.status === "cancelled") &&
				Date.parse(t.created) < cutoff
			) {
				deps.store.archive(t.id);
			}
		}

		const tasks = deps.store.list();
		const live = buildLiveState(deps.registry.list(), tasks, (cwd) =>
			this.laneOfCwd(cwd),
		);
		const decision = schedule(tasks, live, {
			maxConcurrent: deps.config.maxConcurrentTasks,
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

	private async refreshWorktreeCache(): Promise<void> {
		for (const project of this.deps.config.projects) {
			try {
				this.worktreeCache.set(
					project.name,
					await this.deps.resolverIO.listWorktrees(project.path),
				);
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
			// Branch→PR-{number,url} map for this repo, fetched at most ONCE per
			// sweep and only when at least one worktree here is actually being
			// refreshed (all within TTL → no gh call at all). Lazy so a repo served
			// entirely from cache costs nothing. `undefined` = not yet fetched this
			// sweep.
			let prMap:
				| Map<string, { number: number; url: string | null }>
				| null
				| undefined;
			const repoPath = this.repoPath(repo);
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
				const facts = await this.computeGitEnrichment(wt.path);
				const pr = prMap?.get(wt.branch) ?? null;
				const prNumber = pr?.number ?? null;
				const prUrl = pr?.url ?? null;
				const e: GitEnrichment = { ...facts, prNumber, prUrl };
				this.gitEnrichFetchedAt.set(wt.path, Date.now());
				const prev = this.gitEnrichCache.get(wt.path);
				if (
					!prev ||
					prev.dirty !== e.dirty ||
					prev.lastCommitEpoch !== e.lastCommitEpoch ||
					prev.lastCommitAuthor !== e.lastCommitAuthor ||
					prev.lastCommitAuthorEmail !== e.lastCommitAuthorEmail ||
					prev.lastCommitHash !== e.lastCommitHash ||
					prev.prNumber !== e.prNumber ||
					prev.prUrl !== e.prUrl
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

	private async computeGitEnrichment(path: string): Promise<GitCommitFacts> {
		const dirty = await this.gitDirty(path);
		const {
			epoch: lastCommitEpoch,
			author: lastCommitAuthor,
			email: lastCommitAuthorEmail,
			hash: lastCommitHash,
		} = await this.gitLastCommit(path);
		return {
			dirty,
			lastCommitEpoch,
			lastCommitAuthor,
			lastCommitAuthorEmail,
			lastCommitHash,
		};
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
	 * Open PRs for a repo as a branch→{number,url} map, via ONE `gh pr list` call
	 * at the repo root. Any failure — gh missing, unauthenticated, non-zero exit,
	 * unparseable JSON — is treated as "no data" and returns null (never throws).
	 * A row with a non-string `url` keeps its number but stamps url null (gh
	 * always sends it, so this only guards a malformed/forward-compat payload).
	 * Logged at most once per call at debug so a gh-less machine doesn't spam.
	 */
	private async ghPrMap(
		repoPath: string,
	): Promise<Map<string, { number: number; url: string | null }> | null> {
		try {
			const { stdout, exitCode } = await this.deps.exec(
				"gh",
				[
					"pr",
					"list",
					"--state",
					"open",
					"--json",
					"number,headRefName,url",
					"--limit",
					"200",
				],
				{ cwd: repoPath },
			);
			if (exitCode !== 0) {
				console.debug?.("gh pr list: non-zero exit; skipping PR enrichment");
				return null;
			}
			const rows: unknown = JSON.parse(stdout);
			if (!Array.isArray(rows)) return null;
			const map = new Map<string, { number: number; url: string | null }>();
			for (const row of rows) {
				if (row === null || typeof row !== "object") continue;
				const { headRefName, number, url } = row as {
					headRefName?: unknown;
					number?: unknown;
					url?: unknown;
				};
				if (typeof headRefName === "string" && typeof number === "number") {
					map.set(headRefName, {
						number,
						url: typeof url === "string" ? url : null,
					});
				}
			}
			return map;
		} catch {
			console.debug?.("gh pr list: unavailable; skipping PR enrichment");
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
			const resolution = await resolveTarget(
				task.target.ref,
				{ repoPath, tempName: () => qooTempName(task.prompt) },
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

	private startWorker(task: TaskInstance): void {
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
			return;
		}

		// Effective alias→id table for this task's repo. Computed outside the
		// vars.yaml guard above: loadProjectModels is tolerant (never throws), so a
		// malformed `models:` block only disables the override — it must not add a
		// new way to fail the task.
		const modelTable = effectiveModelTable(
			deps.config.models,
			loadProjectModels(projectWorkspaceDir(deps.config, task.target.repo)),
		);

		const lane = laneKey(task) ?? task.id;
		deps.registry.registerWorker(task.id, lane, process.pid);
		const repoPath = this.repoPath(task.target.repo);
		const promise = runTask(task.id, {
			store: deps.store,
			runStore: deps.runStore,
			exec: deps.exec,
			executeClaude: deps.executeClaude,
			executeVerify: deps.executeVerify,
			redact: deps.redact,
			mainSessions: deps.mainSessions,
			// Builtin vars sit below explicit config vars (which spread last and can
			// override them); hooks rendered by the worker see them too.
			globalVars: {
				project: task.target.repo,
				repo_path: repoPath ?? "",
				...deps.config.vars,
			},
			repoVars,
			onSpawned: (id, pid) => this.childPids.set(id, pid),
			isCancelled: (id) => this.cancelledTaskIds.has(id),
			modelTable,
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
			worktreePath: async (repo, worktree) => {
				const path = this.repoPath(repo);
				if (!path) return null;
				// The `@repo` sentinel resolves to the project's primary checkout.
				if (worktree === REPO_SENTINEL) return path;
				const list = await deps.resolverIO.listWorktrees(path);
				return list.find((w) => w.name === worktree)?.path ?? null;
			},
			defaults: { model: "sonnet", timeoutMs: 1_800_000 },
		})
			.catch((err) => {
				try {
					deps.store.update(task.id, {
						status: "failed",
						error: err instanceof Error ? err.message : String(err),
					});
				} catch {}
			})
			.then(() => {
				this.running.delete(task.id);
				this.childPids.delete(task.id);
				this.cancelledTaskIds.delete(task.id);
				deps.registry.unregisterWorker(task.id);
				deps.onChange?.();
			});
		this.running.set(task.id, promise);
		deps.onChange?.();
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
