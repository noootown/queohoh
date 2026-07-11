import { describe, expect, it } from "vitest";
import type { TaskInstance } from "../task.js";
import { laneKey, parseTaskFile, serializeTaskFile } from "../task.js";

const sample: TaskInstance = {
	id: "01J9XK0000000000000000000A",
	status: "queued",
	definition: "platform/pr-review",
	item: { number: "1423", title: "fix auth" },
	itemKey: "1423",
	target: { repo: "platform", ref: "pr:1423", worktree: null },
	priority: "normal",
	created: "2026-07-08T10:12:00.000Z",
	finishedAt: null,
	source: "mcp",
	ephemeralWorktree: false,
	error: null,
	session: "fresh",
	resumeSessionId: null,
	model: null,
	prompt: "Reply to review comments on PR #1423.\n",
	chainId: null,
	chainSeq: null,
};

describe("task file", () => {
	it("round-trips serialize -> parse", () => {
		expect(parseTaskFile(serializeTaskFile(sample))).toEqual(sample);
	});

	it("round-trips an adhoc task with resolved worktree", () => {
		const adhoc: TaskInstance = {
			...sample,
			definition: null,
			item: null,
			itemKey: null,
			target: { repo: "platform", ref: "temp", worktree: "tmp-fix-x9" },
			ephemeralWorktree: true,
			status: "failed",
			error: "tree left dirty",
		};
		expect(parseTaskFile(serializeTaskFile(adhoc))).toEqual(adhoc);
	});

	it("defaults session to fresh when the key is absent", () => {
		const withoutSession = serializeTaskFile(sample).replace(
			/^session: .*\n/m,
			"",
		);
		expect(withoutSession).not.toContain("session:");
		expect(parseTaskFile(withoutSession).session).toBe("fresh");
	});

	it("round-trips a task with session: main", () => {
		const mainSession: TaskInstance = { ...sample, session: "main" };
		const serialized = serializeTaskFile(mainSession);
		expect(serialized).toContain("session: main");
		expect(parseTaskFile(serialized)).toEqual(mainSession);
	});

	it("rejects an invalid session value", () => {
		const bad = serializeTaskFile(sample).replace(
			"session: fresh",
			"session: warm",
		);
		expect(() => parseTaskFile(bad)).toThrow();
	});

	it("round-trips the cancelled status", () => {
		const cancelled: TaskInstance = {
			...sample,
			status: "cancelled",
			error: "cancelled by user",
		};
		expect(parseTaskFile(serializeTaskFile(cancelled))).toEqual(cancelled);
	});

	it("rejects an invalid status", () => {
		const bad = serializeTaskFile(sample).replace(
			"status: queued",
			"status: wat",
		);
		expect(() => parseTaskFile(bad)).toThrow();
	});

	it("rejects unknown/typo'd meta keys", () => {
		const bad = serializeTaskFile(sample).replace(
			"status: queued",
			"status: queued\nprioirty: high",
		);
		expect(() => parseTaskFile(bad)).toThrow();
	});
});

describe("resume_session_id and model fields", () => {
	it("default to null when absent (legacy task files)", () => {
		// serializeTaskFile(sample) is the file's minimal valid frontmatter
		// fixture; a legacy file simply lacks the resume_session_id/model keys.
		const legacy = serializeTaskFile(sample)
			.replace(/^resume_session_id: .*\n/m, "")
			.replace(/^model: .*\n/m, "");
		const task = parseTaskFile(legacy);
		expect(task.resumeSessionId).toBeNull();
		expect(task.model).toBeNull();
	});

	it("round-trip when set", () => {
		const withFields = {
			...sample,
			resumeSessionId: "c77252c9-11d1-4e68-ab81-f099af529091",
			model: "claude-fable-5",
		};
		const reparsed = parseTaskFile(serializeTaskFile(withFields));
		expect(reparsed.resumeSessionId).toBe(
			"c77252c9-11d1-4e68-ab81-f099af529091",
		);
		expect(reparsed.model).toBe("claude-fable-5");
	});
});

describe("finished_at field", () => {
	it("defaults to null when absent (legacy task files)", () => {
		const legacy = serializeTaskFile(sample).replace(/^finished_at: .*\n/m, "");
		expect(legacy).not.toContain("finished_at:");
		expect(parseTaskFile(legacy).finishedAt).toBeNull();
	});

	it("round-trips when set", () => {
		const withFinish = {
			...sample,
			status: "done" as const,
			finishedAt: "2026-07-08T10:15:30.000Z",
		};
		const reparsed = parseTaskFile(serializeTaskFile(withFinish));
		expect(reparsed.finishedAt).toBe("2026-07-08T10:15:30.000Z");
	});
});

describe("chain_id and chain_seq fields", () => {
	it("default to null when absent (legacy task files)", () => {
		const legacy = serializeTaskFile(sample)
			.replace(/^chain_id: .*\n/m, "")
			.replace(/^chain_seq: .*\n/m, "");
		expect(legacy).not.toContain("chain_id:");
		const task = parseTaskFile(legacy);
		expect(task.chainId).toBeNull();
		expect(task.chainSeq).toBeNull();
	});

	it("round-trips when set (a chain member)", () => {
		const member: TaskInstance = {
			...sample,
			chainId: "01CHAIN000000000000000000A",
			chainSeq: 2,
		};
		const reparsed = parseTaskFile(serializeTaskFile(member));
		expect(reparsed.chainId).toBe("01CHAIN000000000000000000A");
		expect(reparsed.chainSeq).toBe(2);
	});

	it("rejects a negative chain_seq", () => {
		const bad = serializeTaskFile({ ...sample, chainSeq: 0 }).replace(
			"chain_seq: 0",
			"chain_seq: -1",
		);
		expect(() => parseTaskFile(bad)).toThrow();
	});
});

describe("laneKey", () => {
	it("is repo:worktree once resolved, null before", () => {
		expect(laneKey(sample)).toBeNull();
		expect(
			laneKey({
				...sample,
				target: { ...sample.target, worktree: "JUS-1423" },
			}),
		).toBe("platform:JUS-1423");
	});
});
