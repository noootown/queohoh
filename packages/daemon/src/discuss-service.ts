import { randomUUID } from "node:crypto";
import type {
	DiscussAnchor,
	DiscussMeta,
	DiscussStatus,
	DiscussStore,
	ExecuteRunOptions,
	GlobalConfig,
	NewTaskInput,
	ProviderAdapter,
	Redactor,
	RunResult,
	SessionLineageStore,
} from "@queohoh/core";
import {
	buildDiscussTurnPrompt,
	executeRun as defaultExecuteRun,
	findModel,
	groupHead,
	runDiscussTurn,
} from "@queohoh/core";
import type { SettingsStore } from "./settings-store.js";

/** Injectable executeRun seam — matches core's executeRun signature. */
export type DiscussExecuteRun = (
	adapter: ProviderAdapter,
	opts: ExecuteRunOptions,
) => Promise<RunResult>;

/**
 * Queue-side deps for promote_* (ad-hoc enqueue + optional definition run).
 * Optional so discuss-api tests that never exercise promote omit them;
 * production daemon always wires them. Missing → promote methods throw.
 */
export interface DiscussQueueDeps {
	/** QueueStore.create (or a test double). */
	create: (input: NewTaskInput) => { id: string };
	/** Map absolute discuss worktree path → registered {repo, worktree name}. */
	resolveCwd: (
		cwd: string,
	) => Promise<{ repo: string; worktree: string } | null>;
	/**
	 * Instantiate a named definition with positional args. Return null when the
	 * def is not installed in the workspace (example tasks must be copied in —
	 * same operator step as squash-merge). Production wires resolveDefinition +
	 * instantiateDefinition; tests stub or return null to force ad-hoc path.
	 */
	tryRunDefinition?: (opts: {
		repo: string;
		name: string;
		args: string[];
		worktreeName: string;
		worktreePath: string;
	}) => Promise<{ id: string } | null>;
}

/** Last ~8k of the discuss transcript for promote prompts (keeps context tight). */
export const PROMOTE_TRANSCRIPT_TAIL = 8_000;

export interface DiscussServiceDeps {
	store: DiscussStore;
	lineage: SessionLineageStore;
	settings: SettingsStore;
	config: GlobalConfig;
	redact: Redactor;
	/**
	 * Wall-clock ceiling for one discuss turn. Discuss is interactive; 30m is
	 * enough for a long review reply without matching the 3h agent default.
	 */
	timeoutMs?: number;
	/** Injectable for tests; production uses core's real `executeRun`. */
	executeRun?: DiscussExecuteRun;
	/** Injectable for tests that want to stub the whole turn path. */
	runTurnFn?: typeof runDiscussTurn;
	/** Promote into the normal agent queue. Optional outside promote tests/prod. */
	queue?: DiscussQueueDeps;
}

/** Wire shape for juice discuss RPCs (snake_case) — keep stable. */
export type DiscussMetaWire = {
	session_id: string;
	worktree: string;
	provider: string;
	status: DiscussStatus;
	lineage_root: string | null;
	created_at: string;
	updated_at: string;
	last_error: string | null;
	active_turn_id: string | null;
};

export function discussMetaToWire(meta: DiscussMeta): DiscussMetaWire {
	return {
		session_id: meta.sessionId,
		worktree: meta.worktree,
		provider: meta.provider,
		status: meta.status,
		lineage_root: meta.lineageRoot,
		created_at: meta.createdAt,
		updated_at: meta.updatedAt,
		last_error: meta.lastError,
		active_turn_id: meta.activeTurnId,
	};
}

/**
 * In-process owner of reserved review (discuss) turns.
 *
 * Turns do NOT go through the QUEUE shim — juice needs low-latency streaming
 * into a reserved session transcript while the operator stays in the review UI.
 * Concurrency is one running turn per session (`discuss_busy` otherwise) so the
 * provider lineage tip never forks two children from the same tip.
 */
export class DiscussService {
	/** Exposed so tests can pass a typed inject without casting through private. */
	readonly executeRun: DiscussExecuteRun;

	private readonly store: DiscussStore;
	private readonly lineage: SessionLineageStore;
	private readonly settings: SettingsStore;
	private readonly config: GlobalConfig;
	private readonly redact: Redactor;
	private readonly timeoutMs: number;
	private readonly runTurnFn: typeof runDiscussTurn;
	private readonly queue: DiscussQueueDeps | undefined;

	/** sessionId → in-flight turn promise bookkeeping. */
	private readonly inFlight = new Set<string>();
	/** sessionId → child pid from onSpawned (for stop). */
	private readonly pids = new Map<string, number>();
	/** sessionIds the operator asked to stop mid-turn. */
	private readonly cancelled = new Set<string>();

	constructor(deps: DiscussServiceDeps) {
		this.store = deps.store;
		this.lineage = deps.lineage;
		this.settings = deps.settings;
		this.config = deps.config;
		this.redact = deps.redact;
		this.timeoutMs = deps.timeoutMs ?? 30 * 60_000;
		this.executeRun = deps.executeRun ?? defaultExecuteRun;
		this.runTurnFn = deps.runTurnFn ?? runDiscussTurn;
		this.queue = deps.queue;
	}

	/**
	 * Get-or-create the reserved session for (worktree, activeProvider).
	 * Heals a stuck `running` status left by a prior daemon process that died
	 * without clearing in-memory flight state.
	 */
	ensure(worktree: string): DiscussMeta {
		if (!worktree) throw new Error("discuss_ensure: worktree required");
		const provider = this.settings.activeProvider();
		const meta = this.store.ensure(worktree, provider);
		return this.healOrphanRunning(meta);
	}

	/**
	 * Start a turn asynchronously. Returns immediately with `status: "running"`.
	 * Throws `discuss_busy` if this session already has an in-flight turn.
	 */
	startTurn(params: {
		worktree: string;
		prompt: string;
		anchor?: DiscussAnchor;
	}): { session_id: string; turn_id: string; status: "running" } {
		if (!params.worktree) throw new Error("discuss_turn: worktree required");
		if (!params.prompt?.trim())
			throw new Error("discuss_turn: prompt required");

		const meta = this.ensure(params.worktree);
		if (this.inFlight.has(meta.sessionId) || meta.status === "running") {
			throw new Error(
				"discuss_busy: a turn is already running for this session",
			);
		}

		const turnId = randomUUID();
		const model = this.resolveModel(meta.provider);
		const providerCfg = this.config.providers.find(
			(p) => p.name === meta.provider,
		);
		const built = buildDiscussTurnPrompt({
			userPrompt: params.prompt,
			anchor: params.anchor,
		});

		this.inFlight.add(meta.sessionId);
		this.cancelled.delete(meta.sessionId);
		this.store.setStatus(meta.sessionId, "running", null);
		this.store.setActiveTurn(meta.sessionId, turnId);

		// Fire-and-forget: client polls discuss_tail; do not await here.
		void this.runTurn({
			sessionId: meta.sessionId,
			turnId,
			prompt: built.fullPrompt,
			systemPrompt: built.systemPrompt,
			model: model.id,
			provider: meta.provider,
			cwd: meta.worktree,
			bin: providerCfg?.bin,
			extraArgs: providerCfg?.args,
		}).finally(() => {
			this.inFlight.delete(meta.sessionId);
			this.pids.delete(meta.sessionId);
			this.cancelled.delete(meta.sessionId);
		});

		return {
			session_id: meta.sessionId,
			turn_id: turnId,
			status: "running",
		};
	}

	tail(
		sessionId: string,
		cursor?: number,
	): {
		text: string;
		next_cursor: number;
		status: DiscussStatus;
		turn_id: string | null;
		error: string | null;
	} {
		if (!sessionId) throw new Error("discuss_tail: session_id required");
		const meta = this.store.get(sessionId);
		if (!meta) throw new Error(`discuss session not found: ${sessionId}`);
		const { text, nextCursor } = this.store.readTranscript(
			sessionId,
			typeof cursor === "number" && cursor >= 0 ? cursor : 0,
		);
		return {
			text,
			next_cursor: nextCursor,
			status: meta.status,
			turn_id: meta.activeTurnId,
			error: meta.lastError,
		};
	}

	/**
	 * Kill the in-flight turn if any. Session + lineage tip are retained.
	 * The async turn's finally path settles status to idle (cancelled is not
	 * treated as a sticky error).
	 */
	stop(sessionId: string): { status: DiscussStatus } {
		if (!sessionId) throw new Error("discuss_stop: session_id required");
		const meta = this.store.get(sessionId);
		if (!meta) throw new Error(`discuss session not found: ${sessionId}`);

		if (this.inFlight.has(sessionId) || meta.status === "running") {
			this.cancelled.add(sessionId);
			const pid = this.pids.get(sessionId);
			if (typeof pid === "number" && pid > 0) {
				try {
					// Prefer process-group kill (executeRun detaches with setsid);
					// fall back to the bare pid if the group kill fails.
					try {
						process.kill(-pid, "SIGTERM");
					} catch {
						process.kill(pid, "SIGTERM");
					}
				} catch {
					// Process already gone — finally path will still settle.
				}
			}
		}

		const latest = this.store.get(sessionId);
		return { status: latest?.status ?? meta.status };
	}

	/**
	 * Mint a fresh reserved session for (worktree, activeProvider). Cancels any
	 * in-flight turn on the old session so it cannot write into a detatched dir
	 * after the index repoint.
	 */
	reset(worktree: string): DiscussMeta {
		if (!worktree) throw new Error("discuss_reset: worktree required");
		const provider = this.settings.activeProvider();
		// Best-effort stop of the session currently indexed for this key.
		const existing = this.store.ensure(worktree, provider);
		if (
			this.inFlight.has(existing.sessionId) ||
			existing.status === "running"
		) {
			this.stop(existing.sessionId);
		}
		return this.store.reset(worktree, provider);
	}

	/**
	 * Promote the review discussion into a normal full-agent queue task that
	 * implements the agreed fix on the discuss worktree. Prompt is built
	 * server-side from the last ~8k of transcript (+ optional operator note).
	 */
	async promoteFix(
		sessionId: string,
		note?: string,
	): Promise<{ task_id: string }> {
		if (!sessionId) throw new Error("discuss_promote_fix: session_id required");
		const queue = this.requireQueue("discuss_promote_fix");
		const meta = this.requireSession(sessionId);
		const { repo, worktree } = await this.resolveSessionTarget(meta, queue);
		const tail = this.transcriptTail(sessionId);
		const noteBlock =
			typeof note === "string" && note.trim().length > 0
				? `\n## Operator note\n\n${note.trim()}\n`
				: "";
		const prompt = [
			"You are implementing a fix identified during a PR code-review discussion.",
			"Work in this worktree with full agent tools. Implement the agreed change;",
			"do not open a PR or push unless the operator note explicitly asks.",
			noteBlock,
			"## Discussion transcript (tail)",
			"",
			tail.length > 0 ? tail : "(empty transcript — use the operator note)",
			"",
			"Implement the fix. Report what you changed.",
		].join("\n");

		const task = queue.create({
			prompt,
			repo,
			ref: `worktree:${worktree}`,
			source: "tui",
			session: "fresh",
		});
		return { task_id: task.id };
	}

	/**
	 * Promote a review draft into a queued task that posts a GitHub PR comment.
	 *
	 * - When `anchor` has path + line → **inline file review comment** on that
	 *   line (GitHub "Comment on line R…"). Ad-hoc enqueue (not conversation
	 *   `pr-reply`), so we always use the pulls review-comment API.
	 * - Otherwise → conversation comment via workspace `pr-reply` def when
	 *   installed, else ad-hoc `gh pr comment`.
	 *
	 * `draft` is the markdown body to post; when omitted, the transcript tail
	 * is used so the agent can shape a short comment from the discussion.
	 */
	async promotePrReply(
		sessionId: string,
		draft?: string,
		pr?: number,
		anchor?: PromoteCommentAnchor | null,
	): Promise<{ task_id: string }> {
		if (!sessionId)
			throw new Error("discuss_promote_pr_reply: session_id required");
		const queue = this.requireQueue("discuss_promote_pr_reply");
		const meta = this.requireSession(sessionId);
		const { repo, worktree } = await this.resolveSessionTarget(meta, queue);

		const body =
			typeof draft === "string" && draft.trim().length > 0
				? draft.trim()
				: this.transcriptTail(sessionId).trim() ||
					"(no draft — derive a short PR comment from context and post it)";
		const prStr =
			typeof pr === "number" && Number.isFinite(pr) ? String(pr) : "";

		const inline = normalizePromoteAnchor(anchor);
		if (inline) {
			// Inline review comment: always ad-hoc so instructions force the
			// pulls/{n}/comments API (workspace pr-reply is conversation-only).
			const prompt = buildAdHocInlineCommentPrompt(body, prStr, inline);
			const task = queue.create({
				prompt,
				repo,
				ref: `worktree:${worktree}`,
				source: "tui",
				session: "fresh",
			});
			return { task_id: task.id };
		}

		if (queue.tryRunDefinition) {
			const viaDef = await queue.tryRunDefinition({
				repo,
				name: "pr-reply",
				args: [body, prStr],
				worktreeName: worktree,
				worktreePath: meta.worktree,
			});
			if (viaDef) return { task_id: viaDef.id };
		}

		// Ad-hoc fallback when pr-reply is not installed in the workspace.
		const prompt = buildAdHocPrReplyPrompt(body, prStr);
		const task = queue.create({
			prompt,
			repo,
			ref: `worktree:${worktree}`,
			source: "tui",
			session: "fresh",
		});
		return { task_id: task.id };
	}

	// ── internals ──────────────────────────────────────────────────────────

	private requireQueue(rpc: string): DiscussQueueDeps {
		if (!this.queue) {
			throw new Error(
				`${rpc}: queue not available (DiscussService missing queue deps)`,
			);
		}
		return this.queue;
	}

	private requireSession(sessionId: string): DiscussMeta {
		const meta = this.store.get(sessionId);
		if (!meta) throw new Error(`discuss session not found: ${sessionId}`);
		return meta;
	}

	private async resolveSessionTarget(
		meta: DiscussMeta,
		queue: DiscussQueueDeps,
	): Promise<{ repo: string; worktree: string }> {
		const resolved = await queue.resolveCwd(meta.worktree);
		if (!resolved) {
			throw new Error(
				`discuss worktree is not under a registered project: ${meta.worktree}`,
			);
		}
		return resolved;
	}

	/** Last PROMOTE_TRANSCRIPT_TAIL chars of the session transcript. */
	private transcriptTail(sessionId: string): string {
		const { text } = this.store.readTranscript(sessionId, 0);
		if (text.length <= PROMOTE_TRANSCRIPT_TAIL) return text;
		return text.slice(-PROMOTE_TRANSCRIPT_TAIL);
	}

	private async runTurn(opts: {
		sessionId: string;
		turnId: string;
		prompt: string;
		systemPrompt: string;
		model: string;
		provider: string;
		cwd: string;
		bin?: string;
		extraArgs?: string[];
	}): Promise<void> {
		const sessionId = opts.sessionId;
		try {
			// Capture the child pid for discuss_stop without extending runDiscussTurn.
			const executeRun: DiscussExecuteRun = (adapter, runOpts) =>
				this.executeRun(adapter, {
					...runOpts,
					onSpawned: (pid) => {
						this.pids.set(sessionId, pid);
						runOpts.onSpawned?.(pid);
					},
				});

			const result = await this.runTurnFn({
				store: this.store,
				lineage: this.lineage,
				sessionId,
				turnId: opts.turnId,
				prompt: opts.prompt,
				systemPrompt: opts.systemPrompt,
				model: opts.model,
				provider: opts.provider,
				cwd: opts.cwd,
				timeoutMs: this.timeoutMs,
				bin: opts.bin,
				extraArgs: opts.extraArgs,
				redact: this.redact,
				executeRun,
			});

			if (this.cancelled.has(sessionId)) {
				// Operator stop — not a sticky failure.
				this.store.setStatus(sessionId, "idle", null);
			} else if (result.exitCode !== 0) {
				this.store.setStatus(
					sessionId,
					"error",
					result.error ?? `discuss turn failed (exit ${result.exitCode})`,
				);
			} else {
				this.store.setStatus(sessionId, "idle", null);
			}
			this.store.setActiveTurn(sessionId, null);
		} catch (err) {
			const msg = err instanceof Error ? err.message : String(err);
			if (this.cancelled.has(sessionId)) {
				this.store.setStatus(sessionId, "idle", null);
			} else {
				this.store.setStatus(sessionId, "error", msg);
			}
			this.store.setActiveTurn(sessionId, null);
		}
	}

	/**
	 * Active provider's model: prefer the default_models entry for that
	 * provider (operator-chosen default), else the catalog group head
	 * (most powerful). Mirrors resolveModelChain's inject path.
	 */
	private resolveModel(provider: string): { id: string; label: string } {
		const catalog = this.config.catalog;
		const fromDefaults = this.config.defaultModels
			.map((r) => findModel(catalog, r))
			.find((e) => e !== undefined && e.provider === provider);
		const entry = fromDefaults ?? groupHead(catalog, provider);
		if (!entry) {
			throw new Error(`no model for provider: ${provider}`);
		}
		return { id: entry.id, label: entry.label };
	}

	/** Clear orphaned running status when no in-memory turn is tracked. */
	private healOrphanRunning(meta: DiscussMeta): DiscussMeta {
		if (meta.status === "running" && !this.inFlight.has(meta.sessionId)) {
			this.store.setStatus(meta.sessionId, "idle", null);
			this.store.setActiveTurn(meta.sessionId, null);
			return this.store.get(meta.sessionId) ?? meta;
		}
		return meta;
	}
}

/** Optional line target for inline PR review comments (juice [+] anchor). */
export type PromoteCommentAnchor = {
	path: string;
	line: number;
	/** `"old"` → LEFT (base), `"new"` → RIGHT (head). Default RIGHT. */
	side?: "old" | "new" | string;
};

/** Normalize wire anchor; returns null when path/line missing or invalid. */
export function normalizePromoteAnchor(
	anchor?: PromoteCommentAnchor | null,
): { path: string; line: number; side: "LEFT" | "RIGHT" } | null {
	if (!anchor) return null;
	const path = typeof anchor.path === "string" ? anchor.path.trim() : "";
	const line = Number(anchor.line);
	if (!path || !Number.isFinite(line) || line <= 0) return null;
	const sideRaw = (anchor.side ?? "new").toString().toLowerCase();
	const side: "LEFT" | "RIGHT" =
		sideRaw === "old" || sideRaw === "left" ? "LEFT" : "RIGHT";
	return { path, line: Math.floor(line), side };
}

/**
 * Ad-hoc pr-reply instructions used when the workspace `pr-reply` definition
 * is not installed. Mirrors the workspace task prompt: short GFM body, no
 * sycophancy, post via `gh`, never edit files or push.
 *
 * Conversation-level only (`gh pr comment`). For file-line comments see
 * {@link buildAdHocInlineCommentPrompt}.
 */
export function buildAdHocPrReplyPrompt(body: string, pr: string): string {
	const prBlock =
		pr.length > 0
			? `PR number: ${pr}`
			: "PR number: (not provided — run `gh pr view --json number -q .number` first)";
	const prArg = pr.length > 0 ? pr : "$(gh pr view --json number -q .number)";
	return [
		"Post a short GitHub PR conversation comment from the draft below.",
		"",
		"## Rules",
		"- No sycophancy, no 'thanks for the great PR', no sign-offs.",
		"- Use GitHub-flavored markdown only as needed (lists, bold, code).",
		"- Default ceiling: 3 sentences. Never restate the whole review thread.",
		"- Forbidden: edit any file, git commit/push, gh pr edit, subagents.",
		"- The ONLY write is `gh pr comment` (PR conversation — not a file line).",
		"",
		`## ${prBlock}`,
		"",
		"## Comment body (post this; lightly polish if the draft is raw transcript)",
		"",
		body,
		"",
		"## Post",
		"",
		"```bash",
		`gh pr comment ${prArg} --body "$(cat <<'EOF'`,
		body,
		"EOF",
		')"',
		"```",
		"",
		"Confirm with the comment URL or a one-line success. Nothing else.",
	].join("\n");
}

/**
 * Ad-hoc instructions to leave an **inline** PR review comment on a file line
 * (GitHub UI: "Comment on line R…"), not a conversation `gh pr comment`.
 */
export function buildAdHocInlineCommentPrompt(
	body: string,
	pr: string,
	anchor: { path: string; line: number; side: "LEFT" | "RIGHT" },
): string {
	const prBlock =
		pr.length > 0
			? `PR number: ${pr}`
			: "PR number: (not provided — run `gh pr view --json number -q .number` first)";
	const prArg = pr.length > 0 ? pr : "$(gh pr view --json number -q .number)";
	const commentBody = extractCommentBody(body);
	return [
		"Leave an INLINE GitHub pull-request review comment on the given file line.",
		"This must appear on the Files changed view as a line comment (e.g. \"Comment on line R130\"),",
		"NOT as a PR conversation comment.",
		"",
		"## Rules",
		"- No sycophancy, no sign-offs.",
		"- Short GFM only as needed. Default ceiling: 3 sentences.",
		"- Forbidden: edit any file, git commit/push, gh pr edit, subagents.",
		"- Forbidden: `gh pr comment` (that is conversation-level).",
		"- The ONLY write is `POST .../pulls/{n}/comments` via `gh api` (inline).",
		`- Target path: \`${anchor.path}\``,
		`- Target line: ${anchor.line}`,
		`- Target side: ${anchor.side} (LEFT=base/old, RIGHT=head/new)`,
		"",
		`## ${prBlock}`,
		"",
		"## Comment body (post only this text; lightly polish if needed)",
		"",
		commentBody,
		"",
		"## Post (inline review comment)",
		"",
		"```bash",
		`PR=${prArg}`,
		'REPO=$(gh repo view --json nameWithOwner -q .nameWithOwner)',
		'COMMIT=$(gh pr view "$PR" --json headRefOid -q .headRefOid)',
		'gh api "repos/$REPO/pulls/$PR/comments" \\',
		`  -f body="$(cat <<'EOF'`,
		commentBody,
		"EOF",
		')" \\',
		'  -f commit_id="$COMMIT" \\',
		`  -f path='${anchor.path.replace(/'/g, "'\\''")}' \\`,
		`  -F line=${anchor.line} \\`,
		`  -f side=${anchor.side}`,
		"```",
		"",
		"If the API rejects the line (not in the diff), retry once with the nearest",
		"diff line on the same path/side; do not fall back to `gh pr comment`.",
		"Confirm with the comment URL or a one-line success. Nothing else.",
	].join("\n");
}

/**
 * Prefer the operator-facing section of juice's structured promote draft;
 * otherwise return the full draft.
 */
export function extractCommentBody(draft: string): string {
	const text = draft.trim();
	if (!text) return text;
	// juice build_promote_context uses this heading for /comment.
	const markers = [
		"## Comment to post (shape into a PR review comment)",
		"## Comment to post",
		"## Operator request",
	];
	for (const m of markers) {
		const idx = text.indexOf(m);
		if (idx >= 0) {
			let rest = text.slice(idx + m.length).trim();
			// Stop at next ## heading if present.
			const next = rest.search(/\n##\s/);
			if (next >= 0) rest = rest.slice(0, next).trim();
			if (rest) return rest;
		}
	}
	return text;
}
