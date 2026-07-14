import { existsSync, unlinkSync } from "node:fs";
import { createServer, type Server, type Socket } from "node:net";
import { homedir } from "node:os";
import { basename, join } from "node:path";
import type {
	ArgSpec,
	ChainStepInput,
	GlobalConfig,
	QueueStore,
	RunStore,
	SessionEntry,
	SessionRegistry,
	TaskInstance,
	WorktreeInfo,
} from "@queohoh/core";
import {
	buildItemFromArgs,
	DEFAULT_MODEL_ALIASES,
	defaultExec,
	effectiveModelTable,
	globalWorkspaceDir,
	instantiateDefinition,
	listClaudeSessions,
	listDefinitions,
	loadProjectDefaultModel,
	loadProjectGithubId,
	loadProjectModels,
	loadProjectVars,
	projectWorkspaceDir,
	render,
	resolveDefinition,
	resolveModel,
	SessionModeSchema,
} from "@queohoh/core";
import { currentBuildId } from "./build-id.js";
import type { Engine } from "./engine.js";
import { configPath } from "./paths.js";

export interface StateSnapshot {
	tasks: TaskInstance[];
	archivedRecent: TaskInstance[];
	sessions: SessionEntry[];
	running: string[];
	maxConcurrent: number;
	/**
	 * `githubId` is the project's optional author identity from its vars.yaml
	 * `github_id` key (see loadProjectGithubId). The TUI matches it against each
	 * worktree's lastCommitAuthorEmail/lastCommitAuthor to sort "my" worktrees
	 * first. Optional/additive — a project without the setting omits the field,
	 * and old TUIs ignore it.
	 */
	projects: { name: string; githubId?: string }[];
	worktrees: Record<string, WorktreeInfo[]>;
	/**
	 * Fingerprint of the daemon's own build (see build-id.ts), computed once at
	 * startup. The TUI compares it against the on-disk build and self-heals a
	 * stale daemon. Optional because pre-feature daemons never sent it — the TUI
	 * treats `undefined` as stale (it definitionally predates this field).
	 */
	buildId?: string;
}

interface ApiDeps {
	engine: Engine;
	store: QueueStore;
	runStore: RunStore;
	registry: SessionRegistry;
	config: GlobalConfig;
	/**
	 * Root of Claude Code's per-project session dirs. Optional — defaults to
	 * `~/.claude/projects` (resolved once in the constructor). Tests inject a
	 * temp dir so `listSessions` reads fixture transcripts.
	 */
	claudeProjectsDir?: string;
	onMutation: () => void;
	/**
	 * Tears the daemon down so a fresh build can take over. Invoked by the
	 * `shutdown` RPC after its reply flushes. Optional so test harnesses that
	 * never exercise shutdown don't have to wire a process-exiting stub.
	 */
	onShutdown?: () => void;
}

function unregisteredCwdMessage(cwd: string, toplevel: string | null): string {
	const repoPath = toplevel ?? cwd;
	return [
		`no registered project contains: ${cwd}`,
		"Add the repo to ~/.config/queohoh/config.yaml under projects:, then retry:",
		"projects:",
		`  - name: ${basename(repoPath)}`,
		`    path: ${repoPath}`,
	].join("\n");
}

export class ApiServer {
	private server: Server | null = null;
	private subscribers = new Set<Socket>();
	private connections = new Set<Socket>();
	// Fingerprint of the running build, captured once at construction (daemon
	// startup) — see build-id.ts. A rebuild that does not restart the daemon
	// leaves this at the old value, which is how the TUI detects staleness.
	private readonly buildId = currentBuildId();
	private readonly claudeProjectsDir: string;

	constructor(private readonly deps: ApiDeps) {
		this.claudeProjectsDir =
			deps.claudeProjectsDir ?? join(homedir(), ".claude", "projects");
	}

	snapshot(): StateSnapshot {
		return {
			tasks: this.deps.store.list(),
			archivedRecent: this.deps.store.listArchived().slice(-20),
			sessions: this.deps.registry.list(),
			running: this.deps.engine.runningTaskIds(),
			maxConcurrent: this.deps.config.maxConcurrentTasks,
			projects: this.deps.config.projects.map((p) => ({
				name: p.name,
				githubId: loadProjectGithubId(
					projectWorkspaceDir(this.deps.config, p.name),
				),
			})),
			worktrees: this.deps.engine.worktreesByRepo(),
			buildId: this.buildId,
		};
	}

	broadcast(): void {
		const frame = `${JSON.stringify({ event: "state", data: this.snapshot() })}\n`;
		for (const sock of this.subscribers) {
			sock.write(frame);
		}
	}

	listen(sockPath: string): Promise<void> {
		if (existsSync(sockPath)) unlinkSync(sockPath);
		this.server = createServer((socket) => this.handleConnection(socket));
		return new Promise((resolve, reject) => {
			this.server?.once("error", reject);
			this.server?.listen(sockPath, () => resolve());
		});
	}

	close(): Promise<void> {
		for (const sock of this.connections) sock.destroy();
		this.connections.clear();
		this.subscribers.clear();
		return new Promise((resolve) => {
			this.server ? this.server.close(() => resolve()) : resolve();
		});
	}

	private handleConnection(socket: Socket): void {
		this.connections.add(socket);
		let buffer = "";
		socket.on("data", async (chunk) => {
			buffer += chunk.toString();
			const lines = buffer.split("\n");
			buffer = lines.pop() ?? "";
			for (const line of lines) {
				if (!line.trim()) continue;
				await this.handleLine(socket, line);
			}
		});
		socket.on("close", () => {
			this.subscribers.delete(socket);
			this.connections.delete(socket);
		});
		socket.on("error", () => {
			this.subscribers.delete(socket);
			this.connections.delete(socket);
		});
	}

	private async handleLine(socket: Socket, line: string): Promise<void> {
		let req: { id?: unknown; method?: unknown; params?: unknown };
		try {
			req = JSON.parse(line);
		} catch {
			socket.write(`${JSON.stringify({ id: null, error: "bad json" })}\n`);
			return;
		}
		const id = req.id ?? null;
		try {
			const result = await this.dispatch(
				String(req.method),
				(req.params ?? {}) as Record<string, unknown>,
				socket,
			);
			socket.write(`${JSON.stringify({ id, result })}\n`);
		} catch (err) {
			socket.write(
				`${JSON.stringify({ id, error: err instanceof Error ? err.message : String(err) })}\n`,
			);
		}
	}

	private async dispatch(
		method: string,
		params: Record<string, unknown>,
		socket: Socket,
	): Promise<unknown> {
		const { deps } = this;
		switch (method) {
			case "ping":
				return "pong";
			case "settings": {
				// Projects are listed when they override the models table OR set a
				// `default_model`; everyone else is fully described by the built-in
				// `default_model` ("opus") + defaults + global.
				const projects = deps.config.projects
					.map((p) => {
						const dir = projectWorkspaceDir(deps.config, p.name);
						return {
							repo: p.name,
							entries: loadProjectModels(dir),
							default_model: loadProjectDefaultModel(dir),
							source: join(dir, "vars.yaml"),
						};
					})
					.filter((p) => Object.keys(p.entries).length > 0 || p.default_model);
				return {
					models: {
						defaults: DEFAULT_MODEL_ALIASES,
						// Built-in default model an ad-hoc / enqueue run uses when nothing
						// sets one; the TUI launcher preselects this (or a project override).
						default_model: "opus",
						global: { entries: deps.config.models, source: configPath() },
						projects,
					},
				};
			}
			case "state":
				return this.snapshot();
			case "subscribe":
				this.subscribers.add(socket);
				return true;
			case "enqueue": {
				const worktree =
					typeof params.worktree === "string" && params.worktree.length > 0
						? params.worktree
						: undefined;
				const session = SessionModeSchema.default("fresh").parse(
					params.session,
				);
				if (session === "main") {
					console.warn(
						'[queohoh] enqueue session:"main" is deprecated and treated as fresh — pass resume_session_id to pin a session',
					);
				}
				const resumeSessionId =
					typeof params.resume_session_id === "string" &&
					params.resume_session_id.length > 0
						? params.resume_session_id
						: undefined;
				const model =
					typeof params.model === "string" && params.model.length > 0
						? params.model
						: undefined;
				const verify =
					typeof params.verify === "string" && params.verify.length > 0
						? params.verify
						: undefined;
				const timeoutMs =
					typeof params.timeout_ms === "number" ? params.timeout_ms : undefined;
				const cwd =
					typeof params.cwd === "string" && params.cwd.length > 0
						? params.cwd
						: undefined;
				let repo = typeof params.repo === "string" ? params.repo : "";
				let ref = worktree
					? `worktree:${worktree}`
					: String(params.ref ?? "temp");
				if (cwd !== undefined) {
					const resolved = await deps.engine.resolveCwd(cwd);
					if (resolved === null) {
						throw new Error(
							unregisteredCwdMessage(cwd, await deps.engine.gitToplevel(cwd)),
						);
					}
					repo = resolved.repo;
					ref = `worktree:${resolved.worktree}`;
				}
				if (repo.length === 0) {
					throw new Error("enqueue requires repo or cwd");
				}
				const task = deps.store.create({
					prompt: String(params.prompt ?? ""),
					repo,
					ref,
					source: "mcp",
					priority: (params.priority as "low" | "normal" | "high") ?? "normal",
					session: "fresh",
					resumeSessionId,
					model,
					timeoutMs,
					verify,
				});
				deps.onMutation();
				return task;
			}
			case "enqueue_chain": {
				const rawSteps = Array.isArray(params.steps) ? params.steps : [];
				if (rawSteps.length === 0) {
					throw new Error("enqueue_chain requires at least one step");
				}
				const priority =
					(params.priority as "low" | "normal" | "high") ?? "normal";
				const model =
					typeof params.model === "string" && params.model.length > 0
						? params.model
						: undefined;
				const timeoutMs =
					typeof params.timeout_ms === "number" ? params.timeout_ms : undefined;
				const resumeSessionId =
					typeof params.resume_session_id === "string" &&
					params.resume_session_id.length > 0
						? params.resume_session_id
						: undefined;
				const worktree =
					typeof params.worktree === "string" && params.worktree.length > 0
						? params.worktree
						: undefined;
				let repo = typeof params.repo === "string" ? params.repo : "";
				let ref = worktree
					? `worktree:${worktree}`
					: String(params.ref ?? "temp");
				if (typeof params.cwd === "string" && params.cwd.length > 0) {
					const resolved = await deps.engine.resolveCwd(params.cwd);
					if (resolved === null) {
						throw new Error(
							unregisteredCwdMessage(
								params.cwd,
								await deps.engine.gitToplevel(params.cwd),
							),
						);
					}
					repo = resolved.repo;
					ref = `worktree:${resolved.worktree}`;
				}
				if (repo.length === 0) {
					throw new Error("enqueue_chain requires repo or cwd");
				}
				const project = deps.config.projects.find((p) => p.name === repo);
				if (!project) throw new Error(`unknown repo: ${repo}`);
				const projectDir = projectWorkspaceDir(deps.config, repo);
				// Builtin vars sit below explicit config vars, mirroring runDefinition;
				// the exec-time worktree pass fills `{{worktree}}` etc. later.
				const globalVars = {
					project: repo,
					repo_path: project.path,
					...deps.config.vars,
				};
				const repoVars = loadProjectVars(projectDir);
				const steps: ChainStepInput[] = rawSteps.map((raw, i) => {
					const s = (raw ?? {}) as {
						definition?: unknown;
						args?: unknown;
						prompt?: unknown;
						verify?: unknown;
					};
					// Per-step done-condition; a definition step's own `verify` still
					// wins at spawn (worker precedence), matching how `model` behaves.
					const verify =
						typeof s.verify === "string" && s.verify.length > 0
							? s.verify
							: undefined;
					if (typeof s.definition === "string" && s.definition.length > 0) {
						const def = resolveDefinition(deps.config, repo, s.definition);
						const values = Array.isArray(s.args) ? s.args.map(String) : [];
						const item = buildItemFromArgs(def, values);
						return {
							prompt: render(def.prompt, globalVars, repoVars, item),
							definition: `${repo}/${def.name}`,
							item,
							// Chain-level model/timeout apply to prompt steps; a definition
							// step's own model/timeout still win at spawn (worker
							// precedence), matching run_task_definition. Priority is the
							// chain's (shared), applied uniformly by createChain so members
							// schedule together.
							model,
							timeoutMs,
							verify,
						};
					}
					if (typeof s.prompt === "string" && s.prompt.length > 0) {
						return { prompt: s.prompt, model, timeoutMs, verify };
					}
					throw new Error(
						`chain step ${i}: must have either 'definition' or 'prompt'`,
					);
				});
				const created = deps.store.createChain(steps, {
					repo,
					ref,
					source: params.source === "mcp" ? "mcp" : "tui",
					priority,
					resumeSessionId,
				});
				deps.onMutation();
				return created;
			}
			case "definitions": {
				type Summary = {
					repo: string;
					name: string;
					scope: "project" | "global";
					args: ArgSpec[];
					hasDiscovery: boolean;
					cron: string | null;
					description: string | null;
					model: string;
				};
				const out: Summary[] = [];
				for (const project of deps.config.projects) {
					try {
						// Resolve model aliases against the effective per-project table
						// (built-in defaults ← global config.yaml ← project vars.yaml).
						// Computed once per project so both def loops share it.
						const table = effectiveModelTable(
							deps.config.models,
							loadProjectModels(projectWorkspaceDir(deps.config, project.name)),
						);
						// Global defs first, then project-local defs shadow them by name.
						const byName = new Map<string, Summary>();
						for (const def of listDefinitions(
							globalWorkspaceDir(deps.config),
							project.name,
						)) {
							byName.set(def.name, {
								repo: project.name,
								name: def.name,
								scope: "global",
								args: def.args,
								hasDiscovery: def.discovery !== null,
								cron: def.cron,
								description: def.description,
								model: resolveModel(def.model, table),
							});
						}
						for (const def of listDefinitions(
							projectWorkspaceDir(deps.config, project.name),
							project.name,
						)) {
							byName.set(def.name, {
								repo: project.name,
								name: def.name,
								scope: "project",
								args: def.args,
								hasDiscovery: def.discovery !== null,
								cron: def.cron,
								description: def.description,
								model: resolveModel(def.model, table),
							});
						}
						for (const summary of [...byName.values()].sort((a, b) =>
							a.name.localeCompare(b.name),
						)) {
							out.push(summary);
						}
					} catch {}
				}
				return out;
			}
			case "runDefinition": {
				const repo = String(params.repo ?? "");
				const name = String(params.name ?? "");
				const project = deps.config.projects.find((p) => p.name === repo);
				if (!project) throw new Error(`unknown repo: ${repo}`);
				const projectDir = projectWorkspaceDir(deps.config, repo);
				const def = resolveDefinition(deps.config, repo, name);
				const args = (params.args as string[] | undefined) ?? [];
				const source = params.source === "mcp" ? "mcp" : "tui";
				// A def declaring `worktree: repo` is location-critical: it must run in
				// the project's primary checkout (squash-merge checks out the target
				// branch there). The picker's worktree already served its purpose as
				// arg context (`source` etc.); pinning the run to that worktree would
				// land it in the wrong cwd, where it could never succeed — so ignore
				// the worktree override for `repo`-pinned defs.
				const worktree =
					def.worktree !== "repo" &&
					typeof params.worktree === "string" &&
					params.worktree.length > 0
						? params.worktree
						: undefined;
				const resumeSessionId =
					typeof params.resume_session_id === "string" &&
					params.resume_session_id.length > 0
						? params.resume_session_id
						: undefined;
				let refOverride = worktree ? `worktree:${worktree}` : undefined;
				// `ref` pins the run's target when no worktree param is given. It beats
				// the definition's own `worktree:` setting — notably a `worktree: auto`
				// def, which would otherwise extract a branch from a PR/ticket URL in
				// the args even when that URL is reference material, not the
				// destination. Ignored for a location-critical `worktree: repo` def
				// (same as the worktree param), and beaten by cwd below.
				if (
					refOverride === undefined &&
					def.worktree !== "repo" &&
					typeof params.ref === "string" &&
					params.ref.length > 0
				) {
					refOverride = String(params.ref);
				}
				if (typeof params.cwd === "string" && params.cwd.length > 0) {
					const resolved = await deps.engine.resolveCwd(params.cwd);
					if (resolved === null) {
						throw new Error(
							unregisteredCwdMessage(
								params.cwd,
								await deps.engine.gitToplevel(params.cwd),
							),
						);
					}
					if (resolved.repo !== repo) {
						throw new Error(
							`cwd resolves to repo ${resolved.repo}, not ${repo}`,
						);
					}
					refOverride = `worktree:${resolved.worktree}`;
				}
				const created = await instantiateDefinition(
					def,
					args.length > 0
						? { mode: "args", values: args.map(String) }
						: { mode: "discover" },
					{
						store: deps.store,
						exec: defaultExec,
						cwd: projectDir,
						source,
						// Builtin vars sit below explicit config vars so an operator can
						// override them; the target project supplies `repo_path`.
						globalVars: {
							project: repo,
							repo_path: project.path,
							...deps.config.vars,
						},
						repoVars: loadProjectVars(projectDir),
						refOverride,
						resumeSessionId,
					},
				);
				deps.onMutation();
				return created;
			}
			case "definition": {
				const repo = String(params.repo ?? "");
				const name = String(params.name ?? "");
				if (!deps.config.projects.some((p) => p.name === repo)) {
					throw new Error(`unknown repo: ${repo}`);
				}
				const def = resolveDefinition(deps.config, repo, name);
				// Resolve the authored model alias against the effective per-project
				// table (built-in defaults ← global config.yaml ← project vars.yaml) —
				// the exact construction `case "definitions"` uses. The authored
				// `model` is preserved as-is; `modelResolved` carries the concrete id
				// so the detail pane can show `alias → id` when they differ. Unknown
				// names (including full ids) pass through unchanged via resolveModel.
				const table = effectiveModelTable(
					deps.config.models,
					loadProjectModels(projectWorkspaceDir(deps.config, repo)),
				);
				return { ...def, modelResolved: resolveModel(def.model, table) };
			}
			case "retry": {
				const task = this.mustGet(String(params.id));
				// `verify-failed` re-queues like `failed` — both are non-success
				// terminal outcomes a user may want to re-run after a fix.
				if (
					task.status !== "failed" &&
					task.status !== "verify-failed" &&
					task.status !== "needs-input"
				) {
					throw new Error(`cannot retry task in status ${task.status}`);
				}
				const updated = deps.store.update(task.id, {
					status: "queued",
					error: null,
				});
				deps.onMutation();
				return updated;
			}
			case "skip": {
				const task = this.mustGet(String(params.id));
				// Two roles for `skip`, by status:
				//  - a LIVE task (queued / needs-input) → user cancel: mark it
				//    `cancelled` (terminal, stays visible, distinct from `failed`).
				//  - an already-TERMINAL task → dismiss: archive it out of the queue.
				if (task.status === "queued" || task.status === "needs-input") {
					const updated = deps.store.update(task.id, {
						status: "cancelled",
						error: "cancelled by user",
					});
					deps.onMutation();
					return updated;
				}
				if (
					["failed", "verify-failed", "done", "skipped", "cancelled"].includes(
						task.status,
					)
				) {
					deps.store.archive(task.id);
					deps.onMutation();
					return true;
				}
				throw new Error(`cannot skip task in status ${task.status}`);
			}
			case "stop": {
				const task = this.mustGet(String(params.id));
				if (task.status !== "running") {
					throw new Error(`cannot stop task in status ${task.status}`);
				}
				// No onMutation: the status change follows later, when the killed
				// worker settles and the store flips the task to `cancelled` (the
				// engine recorded the Stop so the kill reads as a user cancel).
				deps.engine.stopTask(task.id);
				return true;
			}
			case "setWorktree": {
				const task = this.mustGet(String(params.id));
				if (task.status !== "needs-input") {
					throw new Error(
						`cannot set worktree on task in status ${task.status}`,
					);
				}
				const updated = deps.store.update(task.id, {
					status: "queued",
					error: null,
					target: { ...task.target, worktree: String(params.worktree) },
				});
				deps.onMutation();
				return updated;
			}
			case "removeWorktree": {
				await deps.engine.removeWorktree(
					String(params.repo ?? ""),
					String(params.name ?? ""),
				);
				deps.onMutation();
				return true;
			}
			case "createWorktree": {
				const path = await deps.engine.createWorktree(
					String(params.repo ?? ""),
					String(params.name ?? ""),
				);
				deps.onMutation();
				// `path` lets the TUI open a tmux window in the new worktree; old
				// clients that expected `true` treat any non-error reply as success.
				return { path };
			}
			case "heartbeatInteractive": {
				deps.registry.upsertInteractive(
					String(params.cwd),
					typeof params.pid === "number" ? params.pid : null,
				);
				return true;
			}
			case "runMeta":
				return deps.runStore.readRunMeta(String(params.id));
			case "listSessions": {
				const repo = String(params.repo ?? "");
				const worktree = String(params.worktree ?? "");
				const path = await deps.engine.worktreeAbsPath(repo, worktree);
				if (path === null) {
					throw new Error(`unknown worktree: ${repo}/${worktree}`);
				}
				const infos = listClaudeSessions(this.claudeProjectsDir, path, 5);
				const promptBySession = this.runPromptBySession();
				return {
					sessions: infos.map((s) => ({
						session_id: s.sessionId,
						mtime_ms: Math.round(s.mtimeMs),
						label:
							promptBySession.get(s.sessionId) ??
							s.aiTitle ??
							s.firstPrompt ??
							s.sessionId.slice(0, 8),
					})),
				};
			}
			case "shutdown": {
				// Refuse while work is in flight — the caller (TUI self-heal) only asks
				// when idle, but a task can race in between its check and this call.
				if (deps.engine.runningTaskIds().length > 0) {
					throw new Error("busy: task running");
				}
				// Reply true first; tear down after the response frame has flushed so
				// the client sees success before the socket dies.
				setTimeout(() => deps.onShutdown?.(), 50);
				return true;
			}
			default:
				throw new Error(`unknown method: ${method}`);
		}
	}

	private mustGet(id: string): TaskInstance {
		const task = this.deps.store.get(id);
		if (!task) throw new Error(`task not found: ${id}`);
		return task;
	}

	/**
	 * Reverse index: Claude session_id → first line of the task prompt that
	 * produced it. Built from the run store on demand (no persisted index).
	 * Used as the top label preference for `listSessions`.
	 */
	private runPromptBySession(): Map<string, string> {
		const map = new Map<string, string>();
		for (const taskId of this.deps.runStore.listRunTaskIds()) {
			const data = this.deps.runStore.readRunData(taskId);
			const sid = data?.session_id;
			const prompt = data?.task?.prompt;
			if (typeof sid === "string" && sid !== "" && typeof prompt === "string") {
				const firstLine = (prompt.split("\n", 1)[0] ?? "").trim();
				if (firstLine !== "") map.set(sid, firstLine.slice(0, 120));
			}
		}
		return map;
	}
}
