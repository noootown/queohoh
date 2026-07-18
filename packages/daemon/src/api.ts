import { existsSync, unlinkSync } from "node:fs";
import { createServer, type Server, type Socket } from "node:net";
import { homedir } from "node:os";
import { basename, join } from "node:path";
import type {
	ArgSpec,
	CatalogEntry,
	ChainStepInput,
	GlobalConfig,
	ProviderUsage,
	QueueStore,
	RunStore,
	SessionEntry,
	SessionLineageStore,
	SessionRegistry,
	SessionRow,
	TaskInstance,
	WorktreeInfo,
} from "@queohoh/core";
import {
	buildItemFromArgs,
	defaultExec,
	findModel,
	globalWorkspaceDir,
	instantiateDefinition,
	listClaudeSessions,
	listDefinitions,
	loadProjectDefaultModels,
	loadProjectGithubId,
	loadProjectVars,
	mergeSessionSources,
	modelRef,
	projectWorkspaceDir,
	render,
	resolveDefinition,
	SessionModeSchema,
	unknownModelError,
} from "@queohoh/core";
import { currentBuildId } from "./build-id.js";
import type { Engine } from "./engine.js";
import type { SettingsStore } from "./settings-store.js";

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
	/**
	 * The provider the operator has currently switched to (design spec §4). On
	 * the live broadcast so a `set_active_provider` from one client re-renders
	 * every subscriber. Optional/additive — pre-feature daemons omit it and old
	 * TUIs ignore it.
	 */
	activeProvider?: string;
	/**
	 * Active provider usage sample (design: provider-usage-header). Optional —
	 * old daemons omit; null when no successful sample for the active provider.
	 * Present only when a usagePoller is wired (production daemon).
	 */
	providerUsage?: ProviderUsage | null;
}

interface ApiDeps {
	engine: Engine;
	store: QueueStore;
	runStore: RunStore;
	registry: SessionRegistry;
	config: GlobalConfig;
	/** Persisted operator settings (active provider). Read by the `settings`
	 * RPC + the state snapshot; mutated by `set_active_provider`. */
	settings: SettingsStore;
	/** Session fork + provider tags (providerOf used by listSessions). */
	lineage: SessionLineageStore;
	/**
	 * Root of Claude Code's per-project session dirs. Optional — defaults to
	 * `~/.claude/projects` (resolved once in the constructor). Tests inject a
	 * temp dir so `listSessions` reads fixture transcripts.
	 */
	claudeProjectsDir?: string;
	/**
	 * Active-provider usage poller (design: provider-usage-header). Optional so
	 * unit tests that don't care about usage omit it (snapshot then omits
	 * `providerUsage`). Production daemon always wires one; onChange → broadcast.
	 */
	usagePoller?: {
		snapshot: () => ProviderUsage | null;
		onActiveProviderChanged: () => void;
	};
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

/**
 * `listSessions` raw per-source scan bound: how many candidates EACH source
 * (Claude's on-disk transcripts; the daemon's own run store) may contribute
 * before the two are unioned. Distinct from `SESSIONS_PER_PROVIDER` below —
 * that final cap is applied AFTER the union, so a provider isn't pre-
 * truncated before another provider's sessions get a chance to merge in.
 */
const RAW_SESSION_SCAN_LIMIT = 20;

/** Final per-provider cap on the merged, deduped session list (design spec:
 * "5 per provider"). */
const SESSIONS_PER_PROVIDER = 5;

/** Parses an ISO timestamp (`data.json`'s `started_at`/`finished_at`) into
 * epoch ms for session recency ordering; `undefined`/`null`/malformed → null
 * so the caller can fall back to another field. */
function parseTimestamp(iso: string | null | undefined): number | null {
	if (typeof iso !== "string" || iso === "") return null;
	const ms = Date.parse(iso);
	return Number.isNaN(ms) ? null : ms;
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
		const snap: StateSnapshot = {
			tasks: this.deps.store.list(),
			archivedRecent: this.deps.store.listArchived().slice(-20),
			sessions: this.deps.registry.list(),
			running: this.deps.engine.runningTaskIds(),
			// Per-project cap (see GlobalConfig.maxConcurrentTasks) — not a global total.
			maxConcurrent: this.deps.config.maxConcurrentTasks,
			projects: this.deps.config.projects.map((p) => ({
				name: p.name,
				githubId: loadProjectGithubId(
					projectWorkspaceDir(this.deps.config, p.name),
				),
			})),
			worktrees: this.deps.engine.worktreesByRepo(),
			buildId: this.buildId,
			activeProvider: this.deps.settings.activeProvider(),
		};
		// Only set when a poller is wired so bare tests stay free of the field.
		if (this.deps.usagePoller) {
			snap.providerUsage = this.deps.usagePoller.snapshot();
		}
		return snap;
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
				// A project appears under `default_models.projects` only when its
				// vars.yaml sets a NON-EMPTY `default_models:` override (an empty /
				// all-invalid list reads as unset → global fallback, so it is not an
				// override worth listing). Everyone else is described by the global list.
				const projects = deps.config.projects
					.map((p) => {
						const dir = projectWorkspaceDir(deps.config, p.name);
						return {
							name: p.name,
							default_models: loadProjectDefaultModels(dir),
							source: join(dir, "vars.yaml"),
						};
					})
					.filter(
						(
							p,
						): p is {
							name: string;
							default_models: string[];
							source: string;
						} => p.default_models !== undefined && p.default_models.length > 0,
					);
				return {
					// The merged, provider-precedence-grouped catalog (incl. each
					// entry's `hidden` flag — the TUI filters hidden from pickers but
					// still resolves them when referenced explicitly).
					catalog: deps.config.catalog,
					// The provider the operator is currently switched to (SettingsStore).
					active_provider: deps.settings.activeProvider(),
					default_models: {
						global: deps.config.defaultModels,
						projects,
					},
					// deps.config.providers is already the global-effective set
					// (loadGlobalConfig runs it through effectiveProviders). name +
					// enabled always; optional `bin` when configured (interactive
					// goto uses it for the provider's CLI path, e.g. pinned grok).
					// Per-provider model tiers are gone; models live in the catalog.
					providers: deps.config.providers.map((p) => ({
						name: p.name,
						enabled: p.enabled,
						...(p.bin ? { bin: p.bin } : {}),
					})),
				};
			}
			case "state":
				return this.snapshot();
			case "subscribe":
				this.subscribers.add(socket);
				return true;
			case "set_active_provider": {
				// Switch the operator's active provider (design spec §4). Validates the
				// provider exists AND is enabled — SettingsStore throws the message
				// otherwise, surfaced to the client via the standard error frame — then
				// persists (write-through) and returns the new value. Broadcasting the
				// state snapshot (which now carries `activeProvider`) re-renders every
				// subscriber, including a different client than the one that switched.
				// Order: set settings → notify poller (may sync-publish stale cache +
				// kick async fetch) → broadcast so the UI flips immediately.
				const provider = String(params.provider ?? "");
				const value = deps.settings.setActiveProvider(
					provider,
					deps.config.providers,
				);
				deps.usagePoller?.onActiveProviderChanged();
				this.broadcast();
				return value;
			}
			case "set_cron_enabled": {
				// Pause/resume a definition's cron from the TUI (the `[o]cron` toggle).
				// Keyed `<repo>/<name>` to match the engine's cron dedup key and the
				// `definitions` summary's `cronEnabled`. The def's `cron:` expression
				// on disk is untouched — only the SettingsStore pause-set changes, so
				// this never writes the version-controlled config repo. Broadcasting
				// re-renders every subscriber's TASKS Cron column (dim ⇄ bright).
				const repo = String(params.repo ?? "");
				const name = String(params.name ?? "");
				if (repo.length === 0 || name.length === 0) {
					throw new Error("set_cron_enabled: repo and name are required");
				}
				const enabled = params.enabled === true;
				const value = deps.settings.setCronDisabled(
					`${repo}/${name}`,
					!enabled,
				);
				this.broadcast();
				return value;
			}
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
				// Accept a single ref or an ordered fallback list; every ref is
				// validated against the merged catalog (invalid → the enqueue fails
				// with an `unknown model` / did-you-mean message).
				const model = this.coerceModel(deps.config.catalog, params.model);
				// Explicit TUI dialog pick (new-session/adhoc/create-worktree forms):
				// stamped onto the task so the worker runs EXACTLY this ref, no
				// active-provider re-head, no fallback. Absent on MCP's enqueue_task
				// (which never sends it), so it stays unpinned there.
				const modelPinned = params.model_pinned === true;
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
					modelPinned,
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
				// Chain-level model is stamped onto every step (prompt and
				// definition). Worker resolves `task.model ?? def?.model`, so a
				// chain-level stamp overrides a definition's authored list — same
				// override semantics as enqueue / runDefinition. Accept a single
				// ref or a fallback list; every ref is validated against the catalog.
				const model = this.coerceModel(deps.config.catalog, params.model);
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
					// Per-step done-condition. Unlike model (task-first), verify is
					// still def-first at spawn: a definition step's own `verify`
					// beats this chain-step stamp when the def declares one.
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
							// Chain-level model stamps task.model (worker: task beats
							// def). Timeout still uses def-first precedence in the
							// worker. Priority is the chain's (shared), applied
							// uniformly by createChain so members schedule together.
							model,
							timeoutMs,
							verify,
							lane: def.lane ?? undefined,
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
					/** Whether this def's cron is currently ARMED. Meaningful only when
					 * `cron !== null`; the operator pauses/resumes it via
					 * `set_cron_enabled` (persisted in SettingsStore, not the config
					 * repo). A never-toggled def reads `true`. The TUI dims the Cron
					 * column when this is `false`. */
					cronEnabled: boolean;
					description: string | null;
					/** The def's authored `model:` — a `provider/label` ref, an ordered
					 * fallback list of them, or `null` (no `model:` → resolves against
					 * `default_models` at run time). Forwarded as-authored: there is no
					 * alias table to resolve against anymore (the flat catalog replaced
					 * it); the TUI renders the ref(s). */
					model: string | string[] | null;
					/** The def's `worktree:` setting (schema default "temp"). The TUI's
					 * worktree-scoped task menu keeps only defs that consume the
					 * selected worktree — `worktree !== "repo"` (target override
					 * applies) or a context-fillable arg. */
					worktree: string;
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
								cron: def.cron,
								cronEnabled: !deps.settings.isCronDisabled(
									`${project.name}/${def.name}`,
								),
								description: def.description,
								model: def.model,
								worktree: def.worktree,
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
								cronEnabled: !deps.settings.isCronDisabled(
									`${project.name}/${def.name}`,
								),
								description: def.description,
								model: def.model,
								worktree: def.worktree,
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
				// TUI def-run picker sends a 1-entry exact `provider/label`; same
				// coerce/validate path as enqueue. Stamped onto the task so worker
				// prefers it over the def's authored list.
				const model = this.coerceModel(deps.config.catalog, params.model);
				// The def-run picker always sends a concrete pick alongside
				// `model_pinned: true` (there is no empty "default" head option on
				// that dropdown — see `def_model_field`), so the worker runs
				// EXACTLY this ref: no active-provider re-head, no fallback.
				const modelPinned = params.model_pinned === true;
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
					// Always args mode: zero args fill from declared defaults, a
					// required arg without a default errors with `missing required
					// arg`. Discovery is an explicit verb — `discoverDefinition`.
					{ mode: "args", values: args.map(String) },
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
						model,
						modelPinned,
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
				// The full loaded definition, including its authored `model:` — a
				// `provider/label` ref, an ordered fallback list, or `null` (resolves
				// against `default_models` at run time). There is no alias table to
				// resolve against anymore (the flat catalog replaced it), so the
				// authored ref is forwarded as-is and the TUI renders it.
				return resolveDefinition(deps.config, repo, name);
			}
			case "discoverDefinition": {
				const repo = String(params.repo ?? "");
				const name = String(params.name ?? "");
				const project = deps.config.projects.find((p) => p.name === repo);
				if (!project) throw new Error(`unknown repo: ${repo}`);
				const projectDir = projectWorkspaceDir(deps.config, repo);
				const def = resolveDefinition(deps.config, repo, name);
				const source = params.source === "mcp" ? "mcp" : "tui";
				// The explicit discover verb: run the def's discovery command and
				// fan out one task per fresh item. No worktree/ref/cwd overrides —
				// each item resolves its own ref via the def's `worktree:` setting.
				// `instantiateDefinition` rejects a def without discovery.
				const created = await instantiateDefinition(
					def,
					{ mode: "discover" },
					{
						store: deps.store,
						exec: defaultExec,
						cwd: projectDir,
						source,
						globalVars: {
							project: repo,
							repo_path: project.path,
							...deps.config.vars,
						},
						repoVars: loadProjectVars(projectDir),
					},
				);
				deps.onMutation();
				return created;
			}
			case "retry": {
				const task = this.mustGet(String(params.id));
				// Any status re-queues EXCEPT `running`: its in-flight worker owns
				// the status (a settle would clobber the re-queue and the lane
				// would double-run) — stop it first. Terminal successes (done/
				// skipped) revive like failures, and a `queued` retry is an
				// idempotent no-op, so bulk rerun selections never error.
				if (task.status === "running") {
					throw new Error(
						`cannot retry task in status ${task.status} — stop it first`,
					);
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
			case "archive": {
				// The TUI's `[a]rchive` toggle, archive half: dismiss a task out of
				// the live queue. Only `queued`/`running` stay blocked — hiding
				// those would bury live work. A `needs-input` task is PARKED (never
				// started, waiting on a user action), so archiving it hides nothing
				// live and keeps its status intact, so `unarchive` restores it as
				// needs-input — exactly like terminal rows round-trip. Recoverable
				// via `unarchive`.
				const task = this.mustGet(String(params.id));
				if (
					![
						"failed",
						"verify-failed",
						"done",
						"skipped",
						"cancelled",
						"needs-input",
					].includes(task.status)
				) {
					throw new Error(`cannot archive task in status ${task.status}`);
				}
				deps.store.archive(task.id);
				deps.onMutation();
				return true;
			}
			case "unarchive": {
				// The toggle's other half: restore an archived task to the live
				// queue (it re-enters `archivedRecent`-free display with its
				// terminal status intact — nothing re-runs).
				deps.store.unarchive(String(params.id));
				deps.onMutation();
				return true;
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
				const infos = listClaudeSessions(
					this.claudeProjectsDir,
					path,
					RAW_SESSION_SCAN_LIMIT,
				);
				const promptBySession = this.runPromptBySession();
				const modelBySession = this.runModelBySession();
				// Map the run's stored model (the RESOLVED provider-specific id, e.g.
				// `claude-opus-4-8`; see worker.ts) back to its `provider/label` ref so
				// a resumed session can default its launch form to the SAME model it
				// originally ran on (consumed by form.rs). A value already in
				// `provider/label` form passes through; an id/ref the catalog doesn't
				// know (foreign/old sessions) omits `model` → the form falls back to
				// the default.
				const catalog = deps.config.catalog;
				const aliasForModel = (m: string | undefined): string | undefined => {
					if (m === undefined || m === "") return undefined;
					const byId = catalog.find((e) => e.id === m);
					if (byId !== undefined) return modelRef(byId);
					const byRef = catalog.find((e) => modelRef(e) === m);
					return byRef !== undefined ? modelRef(byRef) : undefined;
				};

				// Source A: Claude Code's own on-disk transcripts — the ONLY way to
				// see a claude session started OUTSIDE the daemon (a manual `claude`
				// run in this worktree). Every row here lives under Claude's own
				// transcript dir, so it defaults to "claude" unless a more specific
				// tag is known (a resumed session's model ref, or a lineage tag from
				// a prior daemon spawn that reused this on-disk session).
				const diskRows: SessionRow[] = infos.map((s) => {
					const model = aliasForModel(modelBySession.get(s.sessionId));
					const provider =
						model?.split("/")[0] ??
						deps.lineage.providerOf(s.sessionId) ??
						"claude";
					return {
						sessionId: s.sessionId,
						mtimeMs: s.mtimeMs,
						provider,
						label:
							promptBySession.get(s.sessionId) ??
							s.aiTitle ??
							s.firstPrompt ??
							s.sessionId.slice(0, 8),
						model,
					};
				});

				// Source B: the daemon's OWN run store — records every daemon-
				// launched run across ALL providers (claude, codex, grok, ...) for
				// this worktree. This is the only place a codex/grok session (which
				// never writes into Claude Code's on-disk transcript dir) becomes
				// visible to the picker.
				const rawRunStoreRows: SessionRow[] = [];
				for (const taskId of deps.runStore.listRunTaskIds()) {
					const data = deps.runStore.readRunData(taskId);
					const sessionId = data?.session_id;
					if (
						typeof sessionId !== "string" ||
						sessionId === "" ||
						data?.resolved_worktree_path !== path
					) {
						continue;
					}
					const mtimeMs =
						parseTimestamp(data?.finished_at) ??
						parseTimestamp(data?.started_at) ??
						0;
					// Adoption-safe: a spawn.json/data.json written by an older daemon
					// that predates multi-provider support has no `provider` field —
					// see SpawnSpec.provider's doc comment.
					const provider =
						typeof data?.provider === "string" && data.provider !== ""
							? data.provider
							: "claude";
					rawRunStoreRows.push({
						sessionId,
						mtimeMs,
						provider,
						label: promptBySession.get(sessionId) ?? sessionId.slice(0, 8),
						model: aliasForModel(
							typeof data?.model === "string" ? data.model : undefined,
						),
					});
				}
				// A resumed session can produce several run-store records for the
				// same session id (one per attempt) — dedup + bound per provider
				// BEFORE unioning with disk, distinct from the final per-provider
				// cap applied below.
				const runStoreRows = mergeSessionSources(
					[],
					rawRunStoreRows,
					RAW_SESSION_SCAN_LIMIT,
				);

				// Union both sources, dedup by session id (run-store metadata wins
				// on conflict, max mtime survives), cap to SESSIONS_PER_PROVIDER most
				// recent sessions PER PROVIDER, then merge every provider's survivors
				// into one list sorted by recency — interleaved, not grouped.
				const merged = mergeSessionSources(
					diskRows,
					runStoreRows,
					SESSIONS_PER_PROVIDER,
				);
				return {
					sessions: merged.map((row) => ({
						session_id: row.sessionId,
						mtime_ms: Math.round(row.mtimeMs),
						label: row.label,
						model: row.model,
						provider: row.provider,
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
	 * Coerce and VALIDATE an enqueue `model` param: a single `provider/label`
	 * ref, an ordered fallback list of them, or absent (→ undefined, so the run
	 * resolves against `default_models`). Every ref is checked against the merged
	 * catalog via `findModel`; the first miss throws `unknownModelError` (with a
	 * `did you mean provider/label?` suggestion) so the enqueue fails clearly
	 * rather than creating a task that can never resolve a model. An empty string
	 * or empty list reads as absent, but a NON-empty list with a malformed element
	 * (non-string or empty string, e.g. `[123]` or `[""]`) rejects rather than
	 * silently filtering it out — consistent with the strict unknown-model path.
	 */
	private coerceModel(
		catalog: CatalogEntry[],
		raw: unknown,
	): string | string[] | undefined {
		const validate = (ref: string): void => {
			if (findModel(catalog, ref) === undefined) {
				throw new Error(unknownModelError(catalog, ref));
			}
		};
		if (typeof raw === "string") {
			if (raw.length === 0) return undefined;
			validate(raw);
			return raw;
		}
		if (Array.isArray(raw)) {
			if (raw.length === 0) return undefined;
			const refs: string[] = [];
			for (const r of raw) {
				if (typeof r !== "string" || r.length === 0) {
					throw new Error(
						`invalid model list entry: expected a non-empty "provider/label" ref, got ${JSON.stringify(r)}`,
					);
				}
				validate(r);
				refs.push(r);
			}
			return refs;
		}
		return undefined;
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

	/**
	 * Reverse index: Claude session_id → the model that run used (the resolved
	 * id persisted in run data, e.g. `claude-opus-4-8`). Built from the run
	 * store on demand. `listSessions` maps this back to an alias so resuming a
	 * session defaults to the same model it ran on.
	 */
	private runModelBySession(): Map<string, string> {
		const map = new Map<string, string>();
		for (const taskId of this.deps.runStore.listRunTaskIds()) {
			const data = this.deps.runStore.readRunData(taskId);
			const sid = data?.session_id;
			const model = data?.model;
			if (
				typeof sid === "string" &&
				sid !== "" &&
				typeof model === "string" &&
				model !== ""
			) {
				map.set(sid, model);
			}
		}
		return map;
	}
}
