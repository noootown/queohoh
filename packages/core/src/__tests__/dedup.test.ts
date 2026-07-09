import { describe, expect, it } from "vitest";
import { filterNewItems } from "../dedup.js";
import type { TaskInstance, TaskStatus } from "../task.js";

let seq = 0;
function existing(
	status: TaskStatus,
	itemKey: string,
	definition = "platform/pr-review",
): TaskInstance {
	seq += 1;
	return {
		id: `01DEDUP${String(seq).padStart(19, "0")}`,
		status,
		definition,
		item: { number: itemKey },
		itemKey,
		target: { repo: "platform", ref: `pr:${itemKey}`, worktree: null },
		priority: "normal",
		created: "2026-07-08T00:00:00.000Z",
		source: "cron",
		ephemeralWorktree: false,
		error: null,
		session: "fresh",
		prompt: "p",
	};
}

const items = [{ number: "1" }, { number: "2" }, { number: "3" }];
const base = {
	definition: "platform/pr-review",
	itemKeyTemplate: "{{number}}",
};

describe("filterNewItems", () => {
	it("skip_seen drops keys with any existing instance", () => {
		const out = filterNewItems(items, {
			...base,
			mode: "skip_seen",
			existing: [existing("done", "1"), existing("failed", "2")],
		});
		expect(out).toEqual([{ item: { number: "3" }, itemKey: "3" }]);
	});

	it("retry_errored retries failed-only keys", () => {
		const out = filterNewItems(items, {
			...base,
			mode: "retry_errored",
			existing: [existing("done", "1"), existing("failed", "2")],
		});
		expect(out.map((o) => o.itemKey)).toEqual(["2", "3"]);
	});

	it("retry_errored does not retry a key that also has a live instance", () => {
		const out = filterNewItems([{ number: "1" }], {
			...base,
			mode: "retry_errored",
			existing: [existing("failed", "1"), existing("queued", "1")],
		});
		expect(out).toEqual([]);
	});

	it("none keeps everything with keys", () => {
		const out = filterNewItems([{ number: "9" }], {
			...base,
			mode: "none",
			existing: [existing("done", "9")],
		});
		expect(out).toEqual([{ item: { number: "9" }, itemKey: "9" }]);
	});

	it("only same-definition instances count", () => {
		const out = filterNewItems([{ number: "1" }], {
			...base,
			mode: "skip_seen",
			existing: [existing("done", "1", "platform/other-task")],
		});
		expect(out.map((o) => o.itemKey)).toEqual(["1"]);
	});

	it("throws when item_key does not resolve", () => {
		expect(() =>
			filterNewItems([{ number: "1" }], {
				definition: "d",
				itemKeyTemplate: "{{missing}}",
				mode: "none",
				existing: [],
			}),
		).toThrow("item_key did not resolve: {{missing}}");
	});
});
