import { existsSync, unlinkSync } from "node:fs";
import { createServer, type Server, type Socket } from "node:net";
import { basename } from "node:path";
import type {
	ArgSpec,
	GlobalConfig,
	MainSessionStore,
	QueueStore,
	RunStore,
	SessionEntry,
	SessionRegistry,
	TaskInstance,
	WorktreeInfo,
} from "@queohoh/core";
import {
	defaultExec,
	globalWorkspaceDir,
	instantiateDefinition,
	listDefinitions,
	loadProjectVars,
	projectWorkspaceDir,
	resolveDefinition,
	SessionModeSchema,
} from "@queohoh/core";
import { currentBuildId } from "./build-id.js";
import type { Engine } from "./engine.js";

export interface StateSnapshot {
	tasks: TaskInstance[];
	archivedRecent: TaskInstance[];
	sessions: SessionEntry[];
	running: string[];
	maxConcurrent: number;
	projects: { name: string }[];
	worktrees: Record<string, WorktreeInfo[]>;
	mainSessions: Record<string, string>;
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
	mainSessions: MainSessionStore;
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

	constructor(private readonly deps: ApiDeps) {}

	snapshot(): StateSnapshot {
		return {
			tasks: this.deps.store.list(),
			archivedRecent: this.deps.store.listArchived().slice(-20),
			sessions: this.deps.registry.list(),
			running: this.deps.engine.runningTaskIds(),
			maxConcurrent: this.deps.config.maxConcurrentTasks,
			projects: this.deps.config.projects.map((p) => ({ name: p.name })),
			worktrees: this.deps.engine.worktreesByRepo(),
			mainSessions: this.deps.mainSessions.all(),
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
				const resumeSessionId =
					typeof params.resume_session_id === "string" &&
					params.resume_session_id.length > 0
						? params.resume_session_id
						: undefined;
				const model =
					typeof params.model === "string" && params.model.length > 0
						? params.model
						: undefined;
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
					session,
					resumeSessionId,
					model,
				});
				deps.onMutation();
				return task;
			}
			case "definitions": {
				type Summary = {
					repo: string;
					name: string;
					scope: "project" | "global";
					args: ArgSpec[];
					hasDiscovery: boolean;
				};
				const out: Summary[] = [];
				for (const project of deps.config.projects) {
					try {
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
				const worktree =
					typeof params.worktree === "string" && params.worktree.length > 0
						? params.worktree
						: undefined;
				const resumeSessionId =
					typeof params.resume_session_id === "string" &&
					params.resume_session_id.length > 0
						? params.resume_session_id
						: undefined;
				let refOverride = worktree ? `worktree:${worktree}` : undefined;
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
				return resolveDefinition(deps.config, repo, name);
			}
			case "retry": {
				const task = this.mustGet(String(params.id));
				if (task.status !== "failed" && task.status !== "needs-input") {
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
				if (!["failed", "needs-input", "done"].includes(task.status)) {
					throw new Error(`cannot skip task in status ${task.status}`);
				}
				deps.store.archive(task.id);
				deps.onMutation();
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
				await deps.engine.createWorktree(
					String(params.repo ?? ""),
					String(params.name ?? ""),
				);
				deps.onMutation();
				return true;
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
}
