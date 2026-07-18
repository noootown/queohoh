import { filterNewItems } from "./dedup.js";
import type { TaskDefinition } from "./definition.js";
import { discoverItems } from "./discovery.js";
import { extractRef, formatRef, parseRef } from "./ref.js";
import type { Exec } from "./resolver-io.js";
import type { QueueStore } from "./store.js";
import type { TaskInstance, TaskSource } from "./task.js";
import { render } from "./template.js";

export type Trigger = { mode: "discover" } | { mode: "args"; values: string[] };

/**
 * Reduce positional arg values to the definition's item map. Values are
 * positional and may be shorter than the declared args: trailing args fill from
 * their defaults, a missing value with no default is an error, and any value
 * outside a declared `options` set is rejected. Shared by instantiateDefinition
 * (args mode) and the daemon's chain builder (a `{definition, args}` step).
 */
export function buildItemFromArgs(
	def: TaskDefinition,
	values: string[],
): Record<string, string> {
	if (values.length > def.args.length) {
		const names = def.args.map((a) => a.name).join(", ");
		throw new Error(
			`too many args: expected at most ${def.args.length} (${names}), got ${values.length}`,
		);
	}
	const item: Record<string, string> = {};
	def.args.forEach((spec, i) => {
		let value: string;
		if (i < values.length) {
			value = String(values[i]);
		} else if (spec.default !== undefined) {
			value = spec.default;
		} else {
			throw new Error(`missing required arg: ${spec.name}`);
		}
		if (spec.options && !spec.options.includes(value)) {
			throw new Error(
				`arg ${spec.name}: "${value}" not in options (${spec.options.join(", ")})`,
			);
		}
		item[spec.name] = value;
	});
	return item;
}

export interface InstantiateDeps {
	store: QueueStore;
	exec: Exec;
	cwd: string;
	source: TaskSource;
	globalVars?: Record<string, string>;
	repoVars?: Record<string, string>;
	refOverride?: string;
	resumeSessionId?: string;
	/** Optional model override stamped onto each created task. Worker resolves
	 * `task.model ?? def?.model`, so a TUI def-run exact pick (or enqueue-style
	 * override) beats the definition's authored list when set. Absent → task
	 * keeps `model: null` and the def's list (or `default_models`) applies. */
	model?: string | string[];
	/** True when `model` is an explicit TUI dialog pick that must run EXACTLY
	 * that ref — no active-provider re-head, no fallback chain (see
	 * `TaskInstance.modelPinned`). Absent/false keeps today's re-heading
	 * behavior. */
	modelPinned?: boolean;
	/** True for an explicit TUI dialog def-run — run NOW, ignore dedup. A human
	 * filling the run form and pressing Run means "run this now", even if the
	 * exact item was already seen (possibly failed, possibly still queued
	 * elsewhere) — it must never silently no-op into an empty task list.
	 * Absent/false keeps the definition's configured `dedup` mode. Cron and
	 * MCP-driven runs never set this — they stay deduped. */
	bypassDedup?: boolean;
}

export async function instantiateDefinition(
	def: TaskDefinition,
	trigger: Trigger,
	deps: InstantiateDeps,
): Promise<TaskInstance[]> {
	const globalVars = deps.globalVars ?? {};
	const repoVars = deps.repoVars ?? {};

	let items: Record<string, string>[];
	if (trigger.mode === "discover") {
		if (!def.discovery) {
			throw new Error(`definition ${def.name} has no discovery`);
		}
		// No item vars — items don't exist yet at discovery time.
		items = await discoverItems(
			render(def.discovery.command, globalVars, repoVars),
			deps.exec,
			{ cwd: deps.cwd },
		);
	} else {
		items = [buildItemFromArgs(def, trigger.values)];
	}

	const definition = `${def.repo}/${def.name}`;
	// The discovery item_key template only applies to a discover-mode item — an
	// args-mode item (even on a def that also declares discovery) has no
	// discovery-shaped fields to render it from, so it must key off the declared
	// args instead.
	const itemKeyTemplate =
		trigger.mode === "discover" && def.discovery
			? def.discovery.itemKey
			: defaultKeyTemplate(def);
	const existing = [...deps.store.list(), ...deps.store.listArchived()];
	// A discovery-less cron fire always yields the identical item (from arg
	// defaults / the static `adhoc` key), so `skip_seen` would drop every fire
	// after the first. Fire-timing dedup is owned by the engine's cron cursor, so
	// item dedup is meaningless here — force it off. Discovery-backed crons keep
	// their configured dedup (every-15m pr-fix-ci-conflicts must still skip PRs
// already queued).
	// An explicit TUI dialog def-run (`bypassDedup`) forces it off too: pressing
	// Run is "run NOW" intent and must never silently create zero tasks.
	const dedupMode =
		deps.bypassDedup || (deps.source === "cron" && !def.discovery)
			? "none"
			: def.dedup;
	const fresh = filterNewItems(items, {
		definition,
		itemKeyTemplate,
		mode: dedupMode,
		existing,
	});

	return fresh.map(({ item, itemKey }) =>
		deps.store.create({
			prompt: render(def.prompt, globalVars, repoVars, item),
			repo: def.repo,
			ref: canonicalizeRef(
				resolveRef(def, item, globalVars, repoVars, deps.refOverride),
			),
			source: deps.source,
			priority: def.priority,
			definition,
			item,
			itemKey,
			resumeSessionId: deps.resumeSessionId,
			// Operator/TUI override only — do not copy def.model here; leaving
			// task.model null lets worker fall through to the def's authored list.
			model: deps.model,
			modelPinned: deps.modelPinned ?? false,
			lane: def.lane ?? undefined,
		}),
	);
}

/**
 * Resolve the ref string for a task before canonicalization. `refOverride`
 * (from launching off a worktree row) always wins. Otherwise `worktree: auto`
 * derives the ref from the arg values — the first PR URL / Linear URL / leading
 * ticket found across the item, in declared-arg order — falling back to `temp`;
 * a literal `auto` is never stored. Any other `worktree` value is a template.
 */
function resolveRef(
	def: TaskDefinition,
	item: Record<string, string>,
	globalVars: Record<string, string>,
	repoVars: Record<string, string>,
	refOverride: string | undefined,
): string {
	if (refOverride !== undefined) return refOverride;
	if (def.worktree === "auto") {
		const haystack = def.args.map((a) => item[a.name] ?? "").join("\n");
		return formatRef(extractRef(haystack) ?? { kind: "temp" });
	}
	return render(def.worktree, globalVars, repoVars, item);
}

/**
 * Store the canonical `kind:value` form when the ref parses (so a pasted URL
 * lands as `pr:1821`); leave anything unparseable verbatim for resolution to
 * surface as needs-input later.
 */
function canonicalizeRef(ref: string): string {
	try {
		return formatRef(parseRef(ref));
	} catch {
		return ref;
	}
}

/** Key template when a definition has no discovery block: join declared args. */
function defaultKeyTemplate(def: TaskDefinition): string {
	if (def.args.length === 0) return "adhoc";
	return def.args.map((a) => `{{${a.name}}}`).join(":");
}
