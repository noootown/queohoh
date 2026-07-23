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
	captureModelForSchedule,
	defaultExec,
	DEFER_MS,
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
import type { DiscussService } from "./discuss-service.js";
import { discussMetaToWire } from "./discuss-service.js";
import type { Engine } from "./engine.js";
import type { SettingsStore } from "./settings-store.js";

/**
 * Slice a JS string without splitting a UTF-16 surrogate pair.
 *
 * `String.prototype.slice` counts UTF-16 code units. Cutting between a high
 * and low surrogate leaves an unpaired high in the result; `JSON.stringify`
 * then emits a lone `\ud83d`-style escape, which `serde_json` rejects
 * ("unexpected end of hex escape") and blanks the TUI to an empty snapshot.
 */
export function sliceUtf16Safe(s: string, end: number): string {
	if (end <= 0) return "";
	if (end >= s.length) return s;
	let cut = end;
	// If `cut` sits just after a high surrogate, drop that half-pair.
	const prev = s.charCodeAt(cut - 1);
	if (prev >= 0xd800 && prev <= 0xdbff) cut -= 1;
	return s.slice(0, cut);
}

export interface StateSnapshot {
	tasks: TaskInstance[];
	/**
	 * Full archived task list (not a recent tail). Wire name stays
	 * `archivedRecent` for one-directional compat with older TUIs; the QUEUE
	 * pane filters to the current project and shows every archived row so
	 * full archive history is visible (user request).
	 */
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
	 * Enabled provider names in config-precedence order (`config.providers`
	 * after effectiveProviders, `enabled: true` only). The TUI top-bar chips
	 * render exactly this list — disabled providers never appear. Optional/
	 * additive so old daemons omit it; new TUIs then fall back to the settings
	 * payload's providers.
	 */
	enabledProviders?: string[];
	/**
	 * Active provider usage sample (design: provider-usage-header, single-chip
	 * era). Kept for wire compat with older TUIs that only know the one-chip
	 * shape: when a multi-provider poller is wired this is the active provider's
	 * entry from `providerUsages` (or null). Prefer `providerUsages` for new
	 * clients. Optional — old daemons omit.
	 */
	providerUsage?: ProviderUsage | null;
	/**
	 * Usage samples for every enabled provider the poller has data for, in
	 * config-precedence order (design: provider-usage-header, multi-chip).
	 * Optional — old daemons omit; empty array when the poller has nothing yet.
	 * Present only when a usagePoller is wired (production daemon).
	 */
	providerUsages?: ProviderUsage[];
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
	 * Multi-provider usage poller (design: provider-usage-header). Optional so
	 * unit tests that don't care about usage omit it (snapshot then omits
	 * `providerUsages` / `providerUsage`). Production daemon always wires one;
	 * onChange → broadcast. Polls every enabled provider on its own interval —
	 * provider switches do not kick a refresh.
	 */
	usagePoller?: {
		/** All known samples, in providers() order. */
		snapshot: () => ProviderUsage[];
	};
	/**
	 * Reserved review (discuss) sessions — juice AI review path. Optional so
	 * bare api tests that never exercise discuss_* omit it; production daemon
	 * always wires a DiscussService. Methods throw when called without one.
	 */
	discuss?: DiscussService;
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
		"Add the repo to $QUEOHOH_WORKSPACE/config.yaml under projects:, then retry:",
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

	/**
	 * Wire-side prompt budget. Queue list only needs the first ~line (TUI
	 * `prompt_summary` is 240 chars); full prompts (often multi‑KB intake /
	 * autofix text) were ~75% of a 11 MB snapshot and the main reason state
	 * RPCs took ~6–10s. Detail can re-fetch via the `task` method.
	 */
	static readonly PROMPT_WIRE_MAX = 400;
	/**
	 * Cap on archived rows in the snapshot. Name is still `archivedRecent`;
	 * we keep the newest N by ULID order (ids are time-sortable). Older archive
	 * files stay on disk for unarchive/getAny.
	 */
	static readonly ARCHIVED_WIRE_MAX = 200;

	snapshot(): StateSnapshot {
		const live = this.deps.store.list().map((t) => this.forWire(t));
		// Newest last after id-sort — take the tail.
		const archivedAll = this.deps.store.listArchived();
		const archivedTail =
			archivedAll.length > ApiServer.ARCHIVED_WIRE_MAX
				? archivedAll.slice(-ApiServer.ARCHIVED_WIRE_MAX)
				: archivedAll;
		const snap: StateSnapshot = {
			tasks: live,
			archivedRecent: archivedTail.map((t) => this.forWire(t)),
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
			// Same source the UsagePoller / settings RPC use — enabled only, in
			// config precedence. Top-bar chips must not invent providers.
			enabledProviders: this.deps.config.providers
				.filter((p) => p.enabled)
				.map((p) => p.name),
		};
		// Only set when a poller is wired so bare tests stay free of the fields.
		if (this.deps.usagePoller) {
			const enabled = new Set(snap.enabledProviders ?? []);
			// Never publish samples for a disabled provider (stale cache after
			// a config flip that disabled it mid-process).
			const usages = this.deps.usagePoller
				.snapshot()
				.filter((u) => enabled.has(u.provider));
			snap.providerUsages = usages;
			// Single-chip back-compat: active provider's sample (or null).
			const active = this.deps.settings.activeProvider();
			snap.providerUsage = usages.find((u) => u.provider === active) ?? null;
		}
		return snap;
	}

	/** Truncate heavy fields for the broadcast/state payload. Disk + getAny keep full text. */
	private forWire(task: TaskInstance): TaskInstance {
		const prompt = task.prompt ?? "";
		const verifyOutput = task.verifyOutput ?? null;
		let next = task;
		if (prompt.length > ApiServer.PROMPT_WIRE_MAX) {
			next = {
				...next,
				// UTF-16-safe: a mid-emoji slice leaves an unpaired high surrogate;
				// JSON.stringify emits `\ud83d` and serde_json rejects the frame
				// ("unexpected end of hex escape"), blanking the TUI to defaults.
				prompt: `${sliceUtf16Safe(prompt, ApiServer.PROMPT_WIRE_MAX - 1)}…`,
			};
		}
		// verify_output is bounded ~4 KB already on write; still clamp in case.
		if (verifyOutput !== null && verifyOutput.length > 2048) {
			next = {
				...next,
				verifyOutput: `${sliceUtf16Safe(verifyOutput, 2047)}…`,
			};
		}
		return next;
	}

	broadcast(): void {
		const frame = `${JSON.stringify({ event: "state", data: this.snapshot() })}\n`;
		// Skip write when nothing changed — common for the 2s tick when the
		// queue is idle. Still pays stringify cost, but avoids flooding clients
		// with identical  multi‑MB frames (and the TUI re-derive work).
		if (frame === this.lastBroadcastFrame) return;
		this.lastBroadcastFrame = frame;
		for (const sock of this.subscribers) {
			sock.write(frame);
		}
	}

	/** Last JSON frame sent to subscribers; used to skip no-op broadcasts. */
	private lastBroadcastFrame = "";

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
					// still resolves them when referenced explicitly). Still on the
					// wire for pickers; the settings overlay no longer lists it.
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
					// Important global config.yaml knobs the operator greps for when
					// something "feels wrong" (concurrency, retention, workspace).
					// Additive — old TUIs ignore unknown keys.
					workspace: deps.config.workspace,
					max_concurrent_tasks: deps.config.maxConcurrentTasks,
					purge_after_days: deps.config.purgeAfterDays,
					projects: deps.config.projects.map((p) => p.name),
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
				// Usage is polled for all enabled providers on a timer, so a switch does
				// not kick a fetch — the header already has every sample.
				const provider = String(params.provider ?? "");
				const value = deps.settings.setActiveProvider(
					provider,
					deps.config.providers,
				);
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
				// Explicit model pick: TUI dialogs send `model_pinned: true`; a
				// single-string `model` (MCP /qoo skill, ad-hoc enqueue) is also
				// treated as a pin for capture (exact ref, no fallback).
				const modelPinned =
					params.model_pinned === true || typeof model === "string";
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
				// Freeze the model under the operator's *current* active provider so
				// a later switch cannot re-head this task while it waits in queue.
				const stamped = this.stampScheduleModel(repo, model, modelPinned);
				const task = deps.store.create({
					prompt: String(params.prompt ?? ""),
					repo,
					ref,
					source: "mcp",
					priority: (params.priority as "low" | "normal" | "high") ?? "normal",
					session: "fresh",
					resumeSessionId,
					model: stamped.model,
					modelPinned: stamped.modelPinned,
					timeoutMs,
					verify,
				});
				// Interactive handoff: tag the resume session's provider from the
				// model ref so the worker (and session picker) don't default an
				// untagged Grok/Codex session to claude.
				this.stampResumeProvider(
					deps.lineage,
					deps.config.catalog,
					resumeSessionId,
					stamped.model,
				);
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
				// Same pin rule as enqueue: single-string model = exact pick on
				// every step. Capture freezes the schedule-time chain either way.
				const modelPinned =
					params.model_pinned === true || typeof model === "string";
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
				// Chain-level model (when set) is captured once and shared by every
				// step. Steps without a chain-level model capture their own def /
				// default under the then-active provider.
				const chainStamp =
					model !== undefined
						? this.stampScheduleModel(repo, model, modelPinned)
						: null;
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
						const stamped =
							chainStamp ??
							this.stampScheduleModel(repo, def.model, false);
						return {
							prompt: render(def.prompt, globalVars, repoVars, item),
							definition: `${repo}/${def.name}`,
							item,
							// Schedule-time stamp on task.model (worker: frozen).
							// Timeout still uses def-first precedence in the worker.
							// Priority is the chain's (shared), applied uniformly by
							// createChain so members schedule together.
							model: stamped.model,
							modelPinned: stamped.modelPinned,
							timeoutMs,
							verify,
							lane: def.lane ?? undefined,
							onDone: def.onDone === "archive" ? "archive" : undefined,
							purgeAfterDays: def.purgeAfterDays ?? undefined,
						};
					}
					if (typeof s.prompt === "string" && s.prompt.length > 0) {
						const stamped =
							chainStamp ?? this.stampScheduleModel(repo, null, false);
						return {
							prompt: s.prompt,
							model: stamped.model,
							modelPinned: stamped.modelPinned,
							timeoutMs,
							verify,
						};
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
				this.stampResumeProvider(
					deps.lineage,
					deps.config.catalog,
					resumeSessionId,
					chainStamp?.model ?? model,
				);
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
				// Explicit TUI dialog def-run only (see `run_definition_cmd`) —
				// pressing Run is "run NOW" intent, so dedup must not silently
				// collapse this call to zero created tasks. MCP's runDefinition
				// call never sends this and stays deduped.
				const bypassDedup = params.bypass_dedup === true;
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
						// Freeze model under the then-active provider at schedule time.
						modelCapture: this.modelCaptureCtx(repo),
						bypassDedup,
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
						// Capture model at discover-schedule time under the then-active
						// provider — same freeze as runDefinition / enqueue.
						modelCapture: this.modelCaptureCtx(repo),
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
				// Clears `notBefore` so a manual re-run is not still blocked by a
				// prior `[d]efer` window.
				if (task.status === "running") {
					throw new Error(
						`cannot retry task in status ${task.status} — stop it first`,
					);
				}
				const updated = deps.store.update(task.id, {
					status: "queued",
					error: null,
					notBefore: null,
				});
				deps.onMutation();
				return updated;
			}
			case "defer": {
				// QUEUE `[d]efer`: push a live task +5h (Claude sliding window).
				// Stacks: a second `d` on an already-deferred task adds another
				// +5h onto the existing future `notBefore` (not "from now"), so
				// operators can walk a task further out without undoing prior
				// pushes. A past/null `notBefore` bases from now — which is also
				// the cancel → re-queue → defer path (skip/retry clear
				// `notBefore`, so the next defer starts the 5h window from 0).
				// Queued → stamp `notBefore` (stays queued; scheduler skips until
				// then). Running → stamp FIRST, then stop: finalizeRun sees the
				// future notBefore + cancel marker and re-queues instead of
				// settling `cancelled`. Terminal / needs-input / archived refuse.
				const task = this.mustGet(String(params.id));
				if (task.status !== "queued" && task.status !== "running") {
					throw new Error(`cannot defer task in status ${task.status}`);
				}
				const now = Date.now();
				const existingMs = task.notBefore
					? Date.parse(task.notBefore)
					: Number.NaN;
				const base =
					!Number.isNaN(existingMs) && existingMs > now ? existingMs : now;
				const until = new Date(base + DEFER_MS).toISOString();
				if (task.status === "queued") {
					const updated = deps.store.update(task.id, { notBefore: until });
					deps.onMutation();
					return updated;
				}
				// Running: stamp intent on disk before the kill so a settle that
				// races a daemon death still re-queues (notBefore survives on the
				// task file; cancel marker on the run dir).
				const updated = deps.store.update(task.id, { notBefore: until });
				deps.engine.stopTask(task.id);
				// No onMutation for the stop half — status flips when the kill
				// settles (queued + notBefore). Broadcast the stamped notBefore
				// so the TUI live column can show the scheduled time immediately.
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
					// Clear `notBefore` so a later re-queue + defer starts the 5h
					// window from now (not stacked onto a cancel-era stamp).
					const updated = deps.store.update(task.id, {
						status: "cancelled",
						error: "cancelled by user",
						notBefore: null,
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
			case "task": {
				// Full task (untruncated prompt) for DETAIL prompt tab / resume.
				// Snapshot only ships a short prompt preview for wire size.
				const id = String(params.id ?? "");
				return deps.store.getAny(id) ?? null;
			}
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
			// ── discuss_* (juice AI review; snake_case wire fields) ──────────
			// Additive RPC surface; qoo-tui does not call these. Meta/tail shapes
			// use snake_case (session_id, next_cursor, …) so juice can keep a
			// simple serde rename_all without camelCase duals.
			case "discuss_ensure": {
				const discuss = this.requireDiscuss();
				const worktree = String(params.worktree ?? "");
				if (!worktree) throw new Error("discuss_ensure: worktree required");
				return discussMetaToWire(discuss.ensure(worktree));
			}
			case "discuss_turn": {
				const discuss = this.requireDiscuss();
				const worktree = String(params.worktree ?? "");
				const prompt = String(params.prompt ?? "");
				const rawAnchor = params.anchor;
				let anchor:
					| {
							path: string;
							side: "old" | "new";
							line: number;
							snippet?: string;
					  }
					| undefined;
				if (rawAnchor !== undefined && rawAnchor !== null) {
					if (typeof rawAnchor !== "object") {
						throw new Error("discuss_turn: anchor must be an object");
					}
					const a = rawAnchor as Record<string, unknown>;
					const side = a.side === "old" || a.side === "new" ? a.side : null;
					if (
						typeof a.path !== "string" ||
						side === null ||
						typeof a.line !== "number"
					) {
						throw new Error(
							"discuss_turn: anchor requires path (string), side (old|new), line (number)",
						);
					}
					anchor = {
						path: a.path,
						side,
						line: a.line,
						...(typeof a.snippet === "string" ? { snippet: a.snippet } : {}),
					};
				}
				return discuss.startTurn({ worktree, prompt, anchor });
			}
			case "discuss_tail": {
				const discuss = this.requireDiscuss();
				const sessionId = String(params.session_id ?? "");
				const cursor =
					typeof params.cursor === "number" ? params.cursor : undefined;
				return discuss.tail(sessionId, cursor);
			}
			case "discuss_stop": {
				const discuss = this.requireDiscuss();
				const sessionId = String(params.session_id ?? "");
				return discuss.stop(sessionId);
			}
			case "discuss_reset": {
				const discuss = this.requireDiscuss();
				const worktree = String(params.worktree ?? "");
				if (!worktree) throw new Error("discuss_reset: worktree required");
				return discussMetaToWire(discuss.reset(worktree));
			}
			case "discuss_promote_fix": {
				const discuss = this.requireDiscuss();
				const sessionId = String(params.session_id ?? "");
				const note = typeof params.note === "string" ? params.note : undefined;
				const result = await discuss.promoteFix(sessionId, note);
				// New queued task — kick a tick/broadcast like enqueue.
				deps.onMutation();
				return result;
			}
			case "discuss_promote_pr_reply": {
				const discuss = this.requireDiscuss();
				const sessionId = String(params.session_id ?? "");
				const draft =
					typeof params.draft === "string" ? params.draft : undefined;
				const pr =
					typeof params.pr === "number"
						? params.pr
						: typeof params.pr === "string" && params.pr.length > 0
							? Number(params.pr)
							: undefined;
				const prNum = pr !== undefined && Number.isFinite(pr) ? pr : undefined;
				// Optional line target from juice [+] — when present, post inline.
				const path =
					typeof params.path === "string" ? params.path : undefined;
				const lineRaw = params.line;
				const line =
					typeof lineRaw === "number"
						? lineRaw
						: typeof lineRaw === "string" && lineRaw.length > 0
							? Number(lineRaw)
							: undefined;
				const side =
					typeof params.side === "string" ? params.side : undefined;
				const anchor =
					path && line !== undefined && Number.isFinite(line)
						? { path, line, side }
						: undefined;
				const result = await discuss.promotePrReply(
					sessionId,
					draft,
					prNum,
					anchor,
				);
				deps.onMutation();
				return result;
			}
			default:
				throw new Error(`unknown method: ${method}`);
		}
	}

	/** Discuss RPCs require a wired DiscussService (production always has one). */
	private requireDiscuss(): DiscussService {
		if (!this.deps.discuss) {
			throw new Error("discuss service not available");
		}
		return this.deps.discuss;
	}

	private mustGet(id: string): TaskInstance {
		const task = this.deps.store.get(id);
		if (!task) throw new Error(`task not found: ${id}`);
		return task;
	}

	/**
	 * When enqueueing a resume of an interactive session, tag that session's
	 * provider from a single-string model ref if lineage has no tag yet. Without
	 * this, the worker defaults untagged resumes to claude (pre-Task-11) and a
	 * Grok interactive handoff would spawn the wrong CLI. Never overwrites an
	 * existing tag. No-op when there is no resume id or model is absent/a list.
	 */
	private stampResumeProvider(
		lineage: SessionLineageStore,
		catalog: CatalogEntry[],
		resumeSessionId: string | undefined,
		model: string | string[] | undefined,
	): void {
		if (resumeSessionId === undefined || typeof model !== "string") return;
		if (lineage.providerOf(resumeSessionId) !== null) return;
		const entry = findModel(catalog, model);
		if (entry === undefined) return;
		lineage.recordProvider(resumeSessionId, entry.provider);
	}

	/**
	 * Effective catalog / providers / default_models / active provider for
	 * schedule-time model capture on `repo` (project vars override global
	 * defaults when non-empty).
	 */
	private modelCaptureCtx(repo: string): {
		catalog: CatalogEntry[];
		providers: GlobalConfig["providers"];
		defaultModels: string[];
		activeProvider: string;
	} {
		const { deps } = this;
		const projectDefaults = loadProjectDefaultModels(
			projectWorkspaceDir(deps.config, repo),
		);
		const defaultModels =
			projectDefaults && projectDefaults.length > 0
				? projectDefaults
				: deps.config.defaultModels;
		return {
			catalog: deps.config.catalog,
			providers: deps.config.providers,
			defaultModels,
			activeProvider: deps.settings.activeProvider(),
		};
	}

	/**
	 * Resolve `model` under the operator's current active provider and return
	 * the freeze stamp for `task.model` / `task.modelPinned`. Throws on unknown
	 * refs or no-runnable-model (enqueue must fail rather than queue a task
	 * that can never run).
	 */
	private stampScheduleModel(
		repo: string,
		model: string | string[] | null | undefined,
		pinned: boolean,
	): { model: string | string[]; modelPinned: boolean } {
		const ctx = this.modelCaptureCtx(repo);
		const result = captureModelForSchedule(
			model ?? null,
			ctx.catalog,
			ctx.providers,
			ctx.defaultModels,
			ctx.activeProvider,
			{ pinned: pinned && typeof model === "string" },
		);
		if (!result.ok) throw new Error(result.error);
		return { model: result.model, modelPinned: result.modelPinned };
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
