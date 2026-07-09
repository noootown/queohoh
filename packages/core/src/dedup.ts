import type { TaskInstance } from "./task.js";
import { render } from "./template.js";

export type DedupMode = "skip_seen" | "retry_errored" | "none";

export interface KeyedItem {
	item: Record<string, string>;
	itemKey: string;
}

export function filterNewItems(
	items: Record<string, string>[],
	opts: {
		definition: string;
		itemKeyTemplate: string;
		mode: DedupMode;
		existing: TaskInstance[];
	},
): KeyedItem[] {
	const keyed = items.map((item) => {
		const itemKey = render(opts.itemKeyTemplate, {}, {}, item);
		if (itemKey.includes("{{")) {
			throw new Error(`item_key did not resolve: ${itemKey}`);
		}
		return { item, itemKey };
	});
	if (opts.mode === "none") return keyed;

	const sameDef = opts.existing.filter((t) => t.definition === opts.definition);
	const seen = new Set(
		sameDef.filter((t) => t.itemKey !== null).map((t) => t.itemKey as string),
	);
	const retryable = new Set<string>();
	if (opts.mode === "retry_errored") {
		for (const key of seen) {
			const forKey = sameDef.filter((t) => t.itemKey === key);
			if (forKey.every((t) => t.status === "failed")) retryable.add(key);
		}
	}
	return keyed.filter(
		({ itemKey }) => !seen.has(itemKey) || retryable.has(itemKey),
	);
}
