import { existsSync, unlinkSync } from "node:fs";
import { createServer, type Server, type Socket } from "node:net";
import type {
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
	instantiateDefinition,
	listDefinitions,
	loadDefinition,
	loadProjectVars,
	projectWorkspaceDir,
	SessionModeSchema,
} from "@queohoh/core";
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
}

interface ApiDeps {
	engine: Engine;
	store: QueueStore;
	runStore: RunStore;
	registry: SessionRegistry;
	config: GlobalConfig;
	mainSessions: MainSessionStore;
	onMutation: () => void;
}

export class ApiServer {
	private server: Server | null = null;
	private subscribers = new Set<Socket>();
	private connections = new Set<Socket>();

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
				const task = deps.store.create({
					prompt: String(params.prompt ?? ""),
					repo: String(params.repo ?? ""),
					ref: worktree ? `worktree:${worktree}` : String(params.ref ?? "temp"),
					source: "mcp",
					priority: (params.priority as "low" | "normal" | "high") ?? "normal",
					session,
				});
				deps.onMutation();
				return task;
			}
			case "definitions": {
				const out: {
					repo: string;
					name: string;
					args: string[];
					hasDiscovery: boolean;
				}[] = [];
				for (const project of deps.config.projects) {
					try {
						for (const def of listDefinitions(
							projectWorkspaceDir(deps.config, project.name),
							project.name,
						)) {
							out.push({
								repo: project.name,
								name: def.name,
								args: def.args,
								hasDiscovery: def.discovery !== null,
							});
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
				const def = loadDefinition(projectDir, repo, name);
				const args = (params.args as string[] | undefined) ?? [];
				const source = params.source === "mcp" ? "mcp" : "tui";
				const worktree =
					typeof params.worktree === "string" && params.worktree.length > 0
						? params.worktree
						: undefined;
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
						globalVars: deps.config.vars,
						repoVars: loadProjectVars(projectDir),
						refOverride: worktree ? `worktree:${worktree}` : undefined,
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
				return loadDefinition(
					projectWorkspaceDir(deps.config, repo),
					repo,
					name,
				);
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
			case "heartbeatInteractive": {
				deps.registry.upsertInteractive(
					String(params.cwd),
					typeof params.pid === "number" ? params.pid : null,
				);
				return true;
			}
			case "runMeta":
				return deps.runStore.readRunMeta(String(params.id));
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
