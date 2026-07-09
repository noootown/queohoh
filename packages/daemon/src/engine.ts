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
	loadDefinition,
	loadProjectVars,
	projectWorkspaceDir,
	resolveTarget,
	runTask,
	schedule,
} from "@queohoh/core";

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

	constructor(private readonly deps: EngineDeps) {}

	runningTaskIds(): string[] {
		return [...this.running.keys()];
	}

	worktreesByRepo(): Record<string, WorktreeInfo[]> {
		return Object.fromEntries(this.worktreeCache);
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
				{ repoPath },
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
		const promise = runTask(task.id, {
			store: deps.store,
			runStore: deps.runStore,
			exec: deps.exec,
			executeClaude: deps.executeClaude,
			redact: deps.redact,
			mainSessions: deps.mainSessions,
			globalVars: deps.config.vars,
			repoVars,
			loadDef: (definition) => {
				const [repo, ...nameParts] = definition.split("/");
				const name = nameParts.join("/");
				if (!repo || !this.repoPath(repo)) return null;
				try {
					return loadDefinition(
						projectWorkspaceDir(this.deps.config, repo),
						repo,
						name,
					);
				} catch {
					return null;
				}
			},
			worktreePath: async (repo, worktree) => {
				const repoPath = this.repoPath(repo);
				if (!repoPath) return null;
				const list = await deps.resolverIO.listWorktrees(repoPath);
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
