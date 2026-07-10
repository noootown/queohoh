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
	WorktreeInfo,
} from "@queohoh/core";
import {
	buildLiveState,
	laneKey,
	loadProjectVars,
	projectWorkspaceDir,
	qooTempName,
	REPO_SENTINEL,
	resolveDefinition,
	resolveTarget,
	runTask,
	schedule,
} from "@queohoh/core";

/** Per-worktree git facts merged onto WorktreeInfo. Each field null = unknown. */
interface GitEnrichment {
	dirty: boolean | null;
	lastCommitEpoch: number | null;
	lastCommitAuthor: string | null;
}

/** Serve last-known enrichment for a worktree this long before re-shelling git. */
const GIT_ENRICH_TTL_MS = 60_000;

export interface EngineDeps {
	store: QueueStore;
	runStore: RunStore;
	registry: SessionRegistry;
	config: GlobalConfig;
	resolverIO: ResolverIO;
	exec: Exec;
	executeClaude: ClaudeExecutor;
	redact: Redactor;
	mainSessions: MainSessionStore;
	onChange?: () => void;
}

export class Engine {
	private running = new Map<string, Promise<void>>();
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
	async createWorktree(repo: string, name: string): Promise<void> {
		const repoPath = this.repoPath(repo);
		if (repoPath === null) throw new Error(`unknown repo: ${repo}`);
		const list = await this.deps.resolverIO.listWorktrees(repoPath);
		if (list.some((w) => w.branch === name || w.name === `${repo}.${name}`)) {
			throw new Error(`worktree already exists: ${name}`);
		}
		await this.deps.resolverIO.spawnWorktree(repoPath, name);
		this.worktreeCache.delete(repo);
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

		// Auto-archive old done tasks.
		const cutoff = Date.now() - deps.config.archiveAfterDays * 86_400_000;
		for (const t of deps.store.list()) {
			if (t.status === "done" && Date.parse(t.created) < cutoff) {
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
				this.worktreeCache.set(project.name, []);
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
		for (const [, list] of this.worktreeCache) {
			for (const wt of list) {
				live.add(wt.path);
				if (
					now - (this.gitEnrichFetchedAt.get(wt.path) ?? 0) <
					GIT_ENRICH_TTL_MS
				)
					continue; // serve last-known within TTL
				const e = await this.computeGitEnrichment(wt.path);
				this.gitEnrichFetchedAt.set(wt.path, Date.now());
				const prev = this.gitEnrichCache.get(wt.path);
				if (
					!prev ||
					prev.dirty !== e.dirty ||
					prev.lastCommitEpoch !== e.lastCommitEpoch ||
					prev.lastCommitAuthor !== e.lastCommitAuthor
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

	private async computeGitEnrichment(path: string): Promise<GitEnrichment> {
		const dirty = await this.gitDirty(path);
		const { epoch: lastCommitEpoch, author: lastCommitAuthor } =
			await this.gitLastCommit(path);
		return { dirty, lastCommitEpoch, lastCommitAuthor };
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
	 * HEAD's commit epoch SECONDS + author name in ONE `git log` call:
	 * `--format=%ct%x09%an` prints "<epoch>\t<author>". Either field is null on
	 * failure or when unparseable/empty.
	 */
	private async gitLastCommit(
		path: string,
	): Promise<{ epoch: number | null; author: string | null }> {
		try {
			const { stdout, exitCode } = await this.deps.exec(
				"git",
				["-C", path, "log", "-1", "--format=%ct%x09%an"],
				{ cwd: path },
			);
			if (exitCode !== 0) return { epoch: null, author: null };
			const [epochRaw, ...authorParts] = stdout.trim().split("\t");
			const n = Number.parseInt(epochRaw ?? "", 10);
			const author = authorParts.join("\t").trim();
			return {
				epoch: Number.isFinite(n) ? n : null,
				author: author.length > 0 ? author : null,
			};
		} catch {
			return { epoch: null, author: null };
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

		const lane = laneKey(task) ?? task.id;
		deps.registry.registerWorker(task.id, lane, process.pid);
		const repoPath = this.repoPath(task.target.repo);
		const promise = runTask(task.id, {
			store: deps.store,
			runStore: deps.runStore,
			exec: deps.exec,
			executeClaude: deps.executeClaude,
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
				deps.registry.unregisterWorker(task.id);
				deps.onChange?.();
			});
		this.running.set(task.id, promise);
		deps.onChange?.();
	}
}
