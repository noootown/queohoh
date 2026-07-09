import { filterNewItems } from "./dedup.js";
import type { TaskDefinition } from "./definition.js";
import { discoverItems } from "./discovery.js";
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
		if (trigger.values.length !== def.args.length) {
			throw new Error(
				`expected ${def.args.length} args (${def.args.join(", ")}), got ${trigger.values.length}`,
			);
		}
		const item: Record<string, string> = {};
		def.args.forEach((name, i) => {
			item[name] = String(trigger.values[i]);
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
			ref: deps.refOverride ?? render(def.worktree, globalVars, repoVars, item),
			source: deps.source,
			priority: def.priority,
			definition,
			item,
			itemKey,
		}),
	);
}

/** Key template when a definition has no discovery block: join declared args. */
function defaultKeyTemplate(def: TaskDefinition): string {
	if (def.args.length === 0) return "adhoc";
	return def.args.map((a) => `{{${a}}}`).join(":");
}
