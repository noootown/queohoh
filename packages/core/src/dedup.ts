import type { TaskInstance, TaskStatus } from "./task.js";
import { render } from "./template.js";

export type DedupMode = "skip_seen" | "retry_errored" | "none";

// Terminal statuses that make a `retry_errored` key eligible to re-enqueue. A
// `failed` run errored; a `cancelled` run was deliberately stopped by the user —
// both mean "no active task owns this and it was never handled", so a fresh
// discovery should pick the key up again ("not now" ≠ "never"). `done` (handled)
// and `verify-failed` (worker claimed success) stay blocking; any non-terminal
// task (queued/running/needs-input) also blocks, since a key is only retryable
// when EVERY task under it is in this set.
const RETRYABLE_STATUSES: ReadonlySet<TaskStatus> = new Set([
	"failed",
	"cancelled",
]);

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
			if (forKey.every((t) => RETRYABLE_STATUSES.has(t.status)))
				retryable.add(key);
		}
	}
	return keyed.filter(
		({ itemKey }) => !seen.has(itemKey) || retryable.has(itemKey),
	);
}
