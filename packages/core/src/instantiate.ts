import { filterNewItems } from "./dedup.js";
import type { TaskDefinition } from "./definition.js";
import { discoverItems } from "./discovery.js";
import { extractRef, formatRef, parseRef } from "./ref.js";
import type { Exec } from "./resolver-io.js";
import type { QueueStore } from "./store.js";
import type { TaskInstance, TaskSource } from "./task.js";
import { render } from "./template.js";

export type Trigger = { mode: "discover" } | { mode: "args"; values: string[] };

export interface InstantiateDeps {
	store: QueueStore;
	exec: Exec;
	cwd: string;
	source: TaskSource;
	globalVars?: Record<string, string>;
	repoVars?: Record<string, string>;
	refOverride?: string;
	resumeSessionId?: string;
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
		const { values } = trigger;
		if (values.length > def.args.length) {
			const names = def.args.map((a) => a.name).join(", ");
			throw new Error(
				`too many args: expected at most ${def.args.length} (${names}), got ${values.length}`,
			);
		}
		const item: Record<string, string> = {};
		// Values are positional and may be shorter than args: the trailing args
		// fill from their defaults. A missing value with no default is an error,
		// and any value outside a declared `options` set is rejected.
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
		items = [item];
	}

	const definition = `${def.repo}/${def.name}`;
	const itemKeyTemplate = def.discovery?.itemKey ?? defaultKeyTemplate(def);
	const existing = [...deps.store.list(), ...deps.store.listArchived()];
	const fresh = filterNewItems(items, {
		definition,
		itemKeyTemplate,
		mode: def.dedup,
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
