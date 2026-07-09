# queohoh Plan A — Core Engine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `packages/core` — the pure-logic heart of queohoh: task model (definitions + instances), file-backed queue store, scheduler, worktree resolver, and template variables. Fully unit-tested; no daemon, no UI, no `claude` execution (those are Plans B–D).

**Architecture:** pnpm workspace monorepo. `packages/core` is pure TypeScript with all side effects behind injected interfaces (exec functions, directories passed in) so every module is unit-testable. Files are the source of truth: task instances are markdown-with-YAML-frontmatter files in a tasks dir; definitions are YAML+prompt folders inside each repo's `.queohoh/tasks/`.

**Tech Stack:** TypeScript (strict, ESM), Node >= 22, pnpm, vitest, biome, zod v4, js-yaml, ulid.

**Spec:** `docs/superpowers/specs/2026-07-08-queohoh-slice1-design.md`

## Global Constraints

- Node >= 22, `"type": "module"` everywhere; TS `strict: true`.
- No hand-rolled validation — all external input (YAML files) parses through zod schemas.
- All daemon-side file writes atomic: write `<path>.tmp`, then `rename`.
- Discovery items and template vars are **flat** `Record<string, string>`; template syntax is `{{key}}` (`\w+` keys), unknown keys left verbatim (agent247 behavior).
- Var precedence (low → high): global vars → repo vars → item vars → reserved vars.
- Task ids are ulids (creation-time sortable). FIFO = ulid ascending.
- Lane key = `"<repo>:<worktree>"`. One running task per lane.
- Ticket convention: branch/worktree named by Linear ticket id, regex `/([A-Z][A-Z0-9]*-\d+)/`.
- Priority bands: `high` > `normal` > `low`; FIFO within band.
- Statuses: `queued | needs-input | running | done | failed`. A `failed` task blocks (pauses) its lane.
- Commit after every green test cycle. No `Co-Authored-By` trailers.

---

### Task 1: Monorepo scaffold

**Files:**
- Create: `package.json`, `pnpm-workspace.yaml`, `tsconfig.base.json`, `biome.json`, `.gitignore`, `.node-version`
- Create: `packages/core/package.json`, `packages/core/tsconfig.json`, `packages/core/vitest.config.ts`
- Create: `packages/core/src/index.ts`, `packages/core/src/__tests__/smoke.test.ts`

**Interfaces:**
- Consumes: nothing.
- Produces: a workspace where `pnpm -F @queohoh/core test`, `pnpm -F @queohoh/core typecheck`, and `pnpm lint` run green. All later tasks add files under `packages/core/src/`.

- [ ] **Step 1: Write workspace + package files**

`package.json` (root):

```json
{
	"name": "queohoh",
	"private": true,
	"type": "module",
	"engines": { "node": ">=22" },
	"scripts": {
		"test": "pnpm -r test",
		"typecheck": "pnpm -r typecheck",
		"lint": "biome check --write .",
		"lint:ci": "biome check ."
	},
	"devDependencies": {
		"@biomejs/biome": "^2.4.7"
	}
}
```

`pnpm-workspace.yaml`:

```yaml
packages:
  - packages/*
```

`tsconfig.base.json`:

```json
{
	"compilerOptions": {
		"target": "ES2023",
		"module": "NodeNext",
		"moduleResolution": "NodeNext",
		"strict": true,
		"noUncheckedIndexedAccess": true,
		"skipLibCheck": true,
		"isolatedModules": true,
		"declaration": true,
		"sourceMap": true
	}
}
```

`biome.json` (copy agent247's at `../247/biome.json`, adjust `files.includes` if present — tabs/double-quotes defaults are fine).

`.gitignore`:

```
node_modules/
dist/
*.tsbuildinfo
```

`.node-version`:

```
22
```

`packages/core/package.json`:

```json
{
	"name": "@queohoh/core",
	"version": "0.1.0",
	"type": "module",
	"main": "./src/index.ts",
	"scripts": {
		"test": "vitest run",
		"test:watch": "vitest",
		"typecheck": "tsc --noEmit"
	},
	"dependencies": {
		"js-yaml": "^4.1.1",
		"ulid": "^3.0.2",
		"zod": "^4.3.6"
	},
	"devDependencies": {
		"@types/js-yaml": "^4.0.9",
		"@types/node": "^25.5.0",
		"typescript": "^6.0.2",
		"vitest": "^4.1.0"
	}
}
```

`packages/core/tsconfig.json`:

```json
{
	"extends": "../../tsconfig.base.json",
	"compilerOptions": { "rootDir": "src", "outDir": "dist" },
	"include": ["src"]
}
```

`packages/core/vitest.config.ts`:

```ts
import { defineConfig } from "vitest/config";

export default defineConfig({
	test: { include: ["src/**/*.test.ts"] },
});
```

`packages/core/src/index.ts`:

```ts
export const CORE_VERSION = "0.1.0";
```

- [ ] **Step 2: Write the smoke test**

`packages/core/src/__tests__/smoke.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { CORE_VERSION } from "../index.js";

describe("workspace", () => {
	it("resolves the core package", () => {
		expect(CORE_VERSION).toBe("0.1.0");
	});
});
```

- [ ] **Step 3: Install and verify**

Run: `pnpm install && pnpm -F @queohoh/core test && pnpm -F @queohoh/core typecheck`
Expected: 1 test PASS, typecheck clean.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "chore: monorepo scaffold with @queohoh/core package"
```

---

### Task 2: Frontmatter utility

**Files:**
- Create: `packages/core/src/frontmatter.ts`
- Test: `packages/core/src/__tests__/frontmatter.test.ts`

**Interfaces:**
- Consumes: js-yaml.
- Produces:
  - `parseFrontmatter(content: string): { meta: Record<string, unknown>; body: string }` — throws `Error("missing frontmatter")` if content doesn't start with `---\n`.
  - `stringifyFrontmatter(meta: Record<string, unknown>, body: string): string` — emits `---\n<yaml>---\n\n<body>` and round-trips through `parseFrontmatter`.

- [ ] **Step 1: Write the failing test**

`packages/core/src/__tests__/frontmatter.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { parseFrontmatter, stringifyFrontmatter } from "../frontmatter.js";

describe("parseFrontmatter", () => {
	it("splits meta and body", () => {
		const { meta, body } = parseFrontmatter(
			"---\nid: abc\nnested:\n  a: 1\n---\n\nDo the thing.\n",
		);
		expect(meta).toEqual({ id: "abc", nested: { a: 1 } });
		expect(body).toBe("Do the thing.\n");
	});

	it("keeps --- inside the body", () => {
		const { body } = parseFrontmatter("---\nid: x\n---\n\na\n---\nb\n");
		expect(body).toBe("a\n---\nb\n");
	});

	it("throws on missing frontmatter", () => {
		expect(() => parseFrontmatter("no frontmatter")).toThrow(
			"missing frontmatter",
		);
	});
});

describe("stringifyFrontmatter", () => {
	it("round-trips", () => {
		const meta = { id: "01ABC", n: 5, arr: ["x"] };
		const body = "Prompt text.\n\n## Attachments\nnone\n";
		const out = stringifyFrontmatter(meta, body);
		const back = parseFrontmatter(out);
		expect(back.meta).toEqual(meta);
		expect(back.body).toBe(body);
	});
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm -F @queohoh/core test -- frontmatter`
Expected: FAIL — cannot find module `../frontmatter.js`.

- [ ] **Step 3: Implement**

`packages/core/src/frontmatter.ts`:

```ts
import yaml from "js-yaml";

const DELIM = "---\n";

export function parseFrontmatter(content: string): {
	meta: Record<string, unknown>;
	body: string;
} {
	if (!content.startsWith(DELIM)) throw new Error("missing frontmatter");
	const end = content.indexOf(`\n${DELIM}`, DELIM.length);
	if (end === -1) throw new Error("missing frontmatter");
	const rawMeta = content.slice(DELIM.length, end + 1);
	const meta = yaml.load(rawMeta) as Record<string, unknown>;
	if (meta === null || typeof meta !== "object" || Array.isArray(meta)) {
		throw new Error("frontmatter is not a mapping");
	}
	// skip the closing delimiter and at most one blank separator line
	let body = content.slice(end + 1 + DELIM.length);
	if (body.startsWith("\n")) body = body.slice(1);
	return { meta, body };
}

export function stringifyFrontmatter(
	meta: Record<string, unknown>,
	body: string,
): string {
	return `${DELIM}${yaml.dump(meta)}${DELIM}\n${body}`;
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `pnpm -F @queohoh/core test -- frontmatter`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add packages/core/src/frontmatter.ts packages/core/src/__tests__/frontmatter.test.ts
git commit -m "feat(core): frontmatter parse/stringify utility"
```

---

### Task 3: Template variables (agent247 port)

**Files:**
- Create: `packages/core/src/template.ts`
- Test: `packages/core/src/__tests__/template.test.ts`

**Interfaces:**
- Consumes: nothing.
- Produces: `render(template: string, globalVars?, repoVars?, itemVars?, reservedVars?): string` — all params `Record<string, string>`, later params win, unknown `{{key}}` left verbatim. (Faithful port of `../247/src/lib/template.ts` with `taskVars` renamed `repoVars`.)

- [ ] **Step 1: Write the failing test**

`packages/core/src/__tests__/template.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { render } from "../template.js";

describe("render", () => {
	it("substitutes {{key}} from merged vars", () => {
		expect(render("pr:{{number}}", {}, {}, { number: "257" })).toBe("pr:257");
	});

	it("applies precedence global < repo < item < reserved", () => {
		expect(
			render(
				"{{v}}",
				{ v: "g" },
				{ v: "r" },
				{ v: "i" },
				{ v: "reserved" },
			),
		).toBe("reserved");
		expect(render("{{v}}", { v: "g" }, { v: "r" })).toBe("r");
	});

	it("leaves unknown keys verbatim", () => {
		expect(render("hi {{nope}}", { v: "x" })).toBe("hi {{nope}}");
	});
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm -F @queohoh/core test -- template`
Expected: FAIL — cannot find module `../template.js`.

- [ ] **Step 3: Implement**

`packages/core/src/template.ts`:

```ts
export function render(
	template: string,
	globalVars: Record<string, string> = {},
	repoVars: Record<string, string> = {},
	itemVars: Record<string, string> = {},
	reservedVars: Record<string, string> = {},
): string {
	const merged = { ...globalVars, ...repoVars, ...itemVars, ...reservedVars };
	return template.replace(/\{\{(\w+)\}\}/g, (match, key) =>
		key in merged ? String(merged[key]) : match,
	);
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `pnpm -F @queohoh/core test -- template`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add packages/core/src/template.ts packages/core/src/__tests__/template.test.ts
git commit -m "feat(core): template variable rendering (agent247 port)"
```

---

### Task 4: Target refs

**Files:**
- Create: `packages/core/src/ref.ts`
- Test: `packages/core/src/__tests__/ref.test.ts`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `type TargetRef = { kind: "pr"; number: number } | { kind: "ticket"; id: string } | { kind: "worktree"; name: string } | { kind: "temp" }`
  - `parseRef(raw: string): TargetRef` — throws `Error(\`invalid ref: ${raw}\`)` on garbage.
  - `formatRef(ref: TargetRef): string` — inverse of parseRef.
  - `extractTicketId(text: string): string | null` — first `/([A-Z][A-Z0-9]*-\d+)/` match.

- [ ] **Step 1: Write the failing test**

`packages/core/src/__tests__/ref.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { extractTicketId, formatRef, parseRef } from "../ref.js";

describe("parseRef", () => {
	it("parses each kind", () => {
		expect(parseRef("pr:1423")).toEqual({ kind: "pr", number: 1423 });
		expect(parseRef("ticket:JUS-1423")).toEqual({
			kind: "ticket",
			id: "JUS-1423",
		});
		expect(parseRef("worktree:main")).toEqual({
			kind: "worktree",
			name: "main",
		});
		expect(parseRef("temp")).toEqual({ kind: "temp" });
	});

	it("rejects garbage", () => {
		expect(() => parseRef("pr:abc")).toThrow("invalid ref: pr:abc");
		expect(() => parseRef("nonsense")).toThrow("invalid ref: nonsense");
	});
});

describe("formatRef", () => {
	it("round-trips", () => {
		for (const raw of ["pr:1423", "ticket:JUS-1423", "worktree:main", "temp"]) {
			expect(formatRef(parseRef(raw))).toBe(raw);
		}
	});
});

describe("extractTicketId", () => {
	it("finds ticket ids in branch names", () => {
		expect(extractTicketId("JUS-1423-fix-auth")).toBe("JUS-1423");
		expect(extractTicketId("feature/ABC2-99")).toBe("ABC2-99");
		expect(extractTicketId("no-ticket-here")).toBeNull();
	});
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm -F @queohoh/core test -- ref`
Expected: FAIL — cannot find module `../ref.js`.

- [ ] **Step 3: Implement**

`packages/core/src/ref.ts`:

```ts
export type TargetRef =
	| { kind: "pr"; number: number }
	| { kind: "ticket"; id: string }
	| { kind: "worktree"; name: string }
	| { kind: "temp" };

const TICKET_RE = /([A-Z][A-Z0-9]*-\d+)/;

export function parseRef(raw: string): TargetRef {
	if (raw === "temp") return { kind: "temp" };
	const [kind, ...rest] = raw.split(":");
	const value = rest.join(":");
	if (kind === "pr" && /^\d+$/.test(value)) {
		return { kind: "pr", number: Number(value) };
	}
	if (kind === "ticket" && TICKET_RE.test(value)) {
		return { kind: "ticket", id: value };
	}
	if (kind === "worktree" && value.length > 0) {
		return { kind: "worktree", name: value };
	}
	throw new Error(`invalid ref: ${raw}`);
}

export function formatRef(ref: TargetRef): string {
	switch (ref.kind) {
		case "pr":
			return `pr:${ref.number}`;
		case "ticket":
			return `ticket:${ref.id}`;
		case "worktree":
			return `worktree:${ref.name}`;
		case "temp":
			return "temp";
	}
}

export function extractTicketId(text: string): string | null {
	return TICKET_RE.exec(text)?.[1] ?? null;
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `pnpm -F @queohoh/core test -- ref`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add packages/core/src/ref.ts packages/core/src/__tests__/ref.test.ts
git commit -m "feat(core): target ref parsing and ticket extraction"
```

---

### Task 5: Task instance model + file serialization

**Files:**
- Create: `packages/core/src/task.ts`
- Test: `packages/core/src/__tests__/task.test.ts`

**Interfaces:**
- Consumes: `parseFrontmatter`/`stringifyFrontmatter` (Task 2).
- Produces:
  - `type TaskStatus = "queued" | "needs-input" | "running" | "done" | "failed"`
  - `type Priority = "low" | "normal" | "high"`
  - `type TaskSource = "mcp" | "tui" | "cron"`
  - `interface TaskInstance { id: string; status: TaskStatus; definition: string | null; item: Record<string, string> | null; itemKey: string | null; target: { repo: string; ref: string; worktree: string | null }; priority: Priority; created: string; source: TaskSource; ephemeralWorktree: boolean; error: string | null; prompt: string }`
  - `parseTaskFile(content: string): TaskInstance` — zod-validated, throws on invalid.
  - `serializeTaskFile(task: TaskInstance): string` — round-trips through `parseTaskFile`.
  - `laneKey(task: TaskInstance): string | null` — `"<repo>:<worktree>"`, null while unresolved.

- [ ] **Step 1: Write the failing test**

`packages/core/src/__tests__/task.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { laneKey, parseTaskFile, serializeTaskFile } from "../task.js";
import type { TaskInstance } from "../task.js";

const sample: TaskInstance = {
	id: "01J9XK0000000000000000000A",
	status: "queued",
	definition: "platform/pr-review",
	item: { number: "1423", title: "fix auth" },
	itemKey: "1423",
	target: { repo: "platform", ref: "pr:1423", worktree: null },
	priority: "normal",
	created: "2026-07-08T10:12:00.000Z",
	source: "mcp",
	ephemeralWorktree: false,
	error: null,
	prompt: "Reply to review comments on PR #1423.\n",
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

	it("rejects an invalid status", () => {
		const bad = serializeTaskFile(sample).replace(
			"status: queued",
			"status: wat",
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm -F @queohoh/core test -- task.test`
Expected: FAIL — cannot find module `../task.js`.

- [ ] **Step 3: Implement**

`packages/core/src/task.ts`:

```ts
import { z } from "zod";
import { parseFrontmatter, stringifyFrontmatter } from "./frontmatter.js";

export const TaskStatusSchema = z.enum([
	"queued",
	"needs-input",
	"running",
	"done",
	"failed",
]);
export type TaskStatus = z.infer<typeof TaskStatusSchema>;

export const PrioritySchema = z.enum(["low", "normal", "high"]);
export type Priority = z.infer<typeof PrioritySchema>;

export const TaskSourceSchema = z.enum(["mcp", "tui", "cron"]);
export type TaskSource = z.infer<typeof TaskSourceSchema>;

const TaskMetaSchema = z.object({
	id: z.string().min(1),
	status: TaskStatusSchema,
	definition: z.string().nullable().default(null),
	item: z.record(z.string(), z.string()).nullable().default(null),
	item_key: z.string().nullable().default(null),
	target: z.object({
		repo: z.string().min(1),
		ref: z.string().min(1),
		worktree: z.string().nullable().default(null),
	}),
	priority: PrioritySchema.default("normal"),
	created: z.string().min(1),
	source: TaskSourceSchema,
	ephemeral_worktree: z.boolean().default(false),
	error: z.string().nullable().default(null),
});

export interface TaskInstance {
	id: string;
	status: TaskStatus;
	definition: string | null;
	item: Record<string, string> | null;
	itemKey: string | null;
	target: { repo: string; ref: string; worktree: string | null };
	priority: Priority;
	created: string;
	source: TaskSource;
	ephemeralWorktree: boolean;
	error: string | null;
	prompt: string;
}

export function parseTaskFile(content: string): TaskInstance {
	const { meta, body } = parseFrontmatter(content);
	const m = TaskMetaSchema.parse(meta);
	return {
		id: m.id,
		status: m.status,
		definition: m.definition,
		item: m.item,
		itemKey: m.item_key,
		target: m.target,
		priority: m.priority,
		created: m.created,
		source: m.source,
		ephemeralWorktree: m.ephemeral_worktree,
		error: m.error,
		prompt: body,
	};
}

export function serializeTaskFile(task: TaskInstance): string {
	const meta = {
		id: task.id,
		status: task.status,
		definition: task.definition,
		item: task.item,
		item_key: task.itemKey,
		target: task.target,
		priority: task.priority,
		created: task.created,
		source: task.source,
		ephemeral_worktree: task.ephemeralWorktree,
		error: task.error,
	};
	return stringifyFrontmatter(meta, task.prompt);
}

export function laneKey(task: TaskInstance): string | null {
	if (task.target.worktree === null) return null;
	return `${task.target.repo}:${task.target.worktree}`;
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `pnpm -F @queohoh/core test -- task.test`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add packages/core/src/task.ts packages/core/src/__tests__/task.test.ts
git commit -m "feat(core): task instance model and file serialization"
```

---

### Task 6: Queue store

**Files:**
- Create: `packages/core/src/store.ts`
- Test: `packages/core/src/__tests__/store.test.ts`

**Interfaces:**
- Consumes: `TaskInstance`, `parseTaskFile`, `serializeTaskFile`, `Priority`, `TaskSource` (Task 5); `ulid`.
- Produces: `class QueueStore`:
  - `constructor(stateDir: string)` — creates `<stateDir>/tasks/` and `<stateDir>/archive/` if missing.
  - `create(input: NewTaskInput): TaskInstance` where `interface NewTaskInput { prompt: string; repo: string; ref: string; source: TaskSource; priority?: Priority; definition?: string; item?: Record<string, string>; itemKey?: string }` — generates ulid id + created timestamp, status `queued`, writes file.
  - `list(): TaskInstance[]` — all live tasks, ulid ascending; skips unparseable files (collects them in `store.invalidFiles: string[]`).
  - `get(id: string): TaskInstance | undefined`
  - `update(id: string, patch: Partial<Omit<TaskInstance, "id">>): TaskInstance` — read-modify-write, **atomic** (tmp + rename); throws `Error(\`task not found: ${id}\`)`.
  - `archive(id: string): void` — moves file to `archive/`.
  - `taskPath(id: string): string`

- [ ] **Step 1: Write the failing test**

`packages/core/src/__tests__/store.test.ts`:

```ts
import { existsSync, mkdtempSync, readdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { QueueStore } from "../store.js";

function freshStore(): QueueStore {
	return new QueueStore(mkdtempSync(join(tmpdir(), "queohoh-store-")));
}

describe("QueueStore", () => {
	it("creates a queued task with generated id and lists it", () => {
		const store = freshStore();
		const t = store.create({
			prompt: "fix the flaky test\n",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		expect(t.status).toBe("queued");
		expect(t.id).toMatch(/^[0-9A-HJKMNP-TV-Z]{26}$/);
		expect(store.list()).toEqual([t]);
		expect(store.get(t.id)).toEqual(t);
	});

	it("lists in ulid (creation) order", () => {
		const store = freshStore();
		const a = store.create({ prompt: "a", repo: "r", ref: "temp", source: "tui" });
		const b = store.create({ prompt: "b", repo: "r", ref: "temp", source: "tui" });
		expect(store.list().map((t) => t.id)).toEqual([a.id, b.id].sort());
	});

	it("update patches and persists atomically", () => {
		const store = freshStore();
		const t = store.create({ prompt: "x", repo: "r", ref: "temp", source: "mcp" });
		const updated = store.update(t.id, {
			status: "failed",
			error: "boom",
			target: { ...t.target, worktree: "tmp-x-1" },
		});
		expect(updated.status).toBe("failed");
		expect(store.get(t.id)?.error).toBe("boom");
		expect(store.get(t.id)?.target.worktree).toBe("tmp-x-1");
		// no stray tmp files left behind
		const dir = join(store.stateDir, "tasks");
		expect(readdirSync(dir).filter((f) => f.endsWith(".tmp"))).toEqual([]);
	});

	it("update throws for unknown id", () => {
		const store = freshStore();
		expect(() => store.update("01UNKNOWN0000000000000000X", {})).toThrow(
			/task not found/,
		);
	});

	it("archive moves the file out of tasks/", () => {
		const store = freshStore();
		const t = store.create({ prompt: "x", repo: "r", ref: "temp", source: "tui" });
		store.archive(t.id);
		expect(store.list()).toEqual([]);
		expect(existsSync(join(store.stateDir, "archive", `${t.id}.md`))).toBe(true);
	});

	it("skips unparseable files and reports them", () => {
		const store = freshStore();
		store.create({ prompt: "good", repo: "r", ref: "temp", source: "tui" });
		writeFileSync(join(store.stateDir, "tasks", "junk.md"), "not a task");
		expect(store.list()).toHaveLength(1);
		expect(store.invalidFiles).toEqual([
			join(store.stateDir, "tasks", "junk.md"),
		]);
	});
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm -F @queohoh/core test -- store`
Expected: FAIL — cannot find module `../store.js`.

- [ ] **Step 3: Implement**

`packages/core/src/store.ts`:

```ts
import {
	mkdirSync,
	readFileSync,
	readdirSync,
	renameSync,
	writeFileSync,
} from "node:fs";
import { join } from "node:path";
import { ulid } from "ulid";
import type { Priority, TaskInstance, TaskSource } from "./task.js";
import { parseTaskFile, serializeTaskFile } from "./task.js";

export interface NewTaskInput {
	prompt: string;
	repo: string;
	ref: string;
	source: TaskSource;
	priority?: Priority;
	definition?: string;
	item?: Record<string, string>;
	itemKey?: string;
}

export class QueueStore {
	readonly stateDir: string;
	readonly tasksDir: string;
	readonly archiveDir: string;
	invalidFiles: string[] = [];

	constructor(stateDir: string) {
		this.stateDir = stateDir;
		this.tasksDir = join(stateDir, "tasks");
		this.archiveDir = join(stateDir, "archive");
		mkdirSync(this.tasksDir, { recursive: true });
		mkdirSync(this.archiveDir, { recursive: true });
	}

	taskPath(id: string): string {
		return join(this.tasksDir, `${id}.md`);
	}

	create(input: NewTaskInput): TaskInstance {
		const task: TaskInstance = {
			id: ulid(),
			status: "queued",
			definition: input.definition ?? null,
			item: input.item ?? null,
			itemKey: input.itemKey ?? null,
			target: { repo: input.repo, ref: input.ref, worktree: null },
			priority: input.priority ?? "normal",
			created: new Date().toISOString(),
			source: input.source,
			ephemeralWorktree: false,
			error: null,
			prompt: input.prompt,
		};
		this.write(task);
		return task;
	}

	list(): TaskInstance[] {
		this.invalidFiles = [];
		const tasks: TaskInstance[] = [];
		for (const file of readdirSync(this.tasksDir).sort()) {
			if (!file.endsWith(".md")) continue;
			const path = join(this.tasksDir, file);
			try {
				tasks.push(parseTaskFile(readFileSync(path, "utf-8")));
			} catch {
				this.invalidFiles.push(path);
			}
		}
		return tasks.sort((a, b) => a.id.localeCompare(b.id));
	}

	get(id: string): TaskInstance | undefined {
		try {
			return parseTaskFile(readFileSync(this.taskPath(id), "utf-8"));
		} catch {
			return undefined;
		}
	}

	update(
		id: string,
		patch: Partial<Omit<TaskInstance, "id">>,
	): TaskInstance {
		const current = this.get(id);
		if (!current) throw new Error(`task not found: ${id}`);
		const next: TaskInstance = { ...current, ...patch, id };
		this.write(next);
		return next;
	}

	archive(id: string): void {
		renameSync(this.taskPath(id), join(this.archiveDir, `${id}.md`));
	}

	private write(task: TaskInstance): void {
		const path = this.taskPath(task.id);
		const tmp = `${path}.tmp`;
		writeFileSync(tmp, serializeTaskFile(task));
		renameSync(tmp, path);
	}
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `pnpm -F @queohoh/core test -- store`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add packages/core/src/store.ts packages/core/src/__tests__/store.test.ts
git commit -m "feat(core): file-backed queue store with atomic writes"
```

---

### Task 7: Duration parsing + task definitions

**Files:**
- Create: `packages/core/src/duration.ts`, `packages/core/src/definition.ts`
- Test: `packages/core/src/__tests__/duration.test.ts`, `packages/core/src/__tests__/definition.test.ts`

**Interfaces:**
- Consumes: js-yaml, zod, `PrioritySchema` (Task 5).
- Produces:
  - `parseDuration(text: string): number` — `"30m" | "2h" | "45s" | "7d"` → ms; throws `Error(\`invalid duration: ${text}\`)`.
  - `interface TaskDefinition { name: string; repo: string; discovery: { command: string; itemKey: string } | null; args: string[]; dedup: "skip_seen" | "retry_errored" | "none"; worktree: string; preRun: string | null; postRun: string | null; model: string; timeoutMs: number; priority: Priority; prompt: string }`
  - `loadDefinition(repoPath: string, repoName: string, taskName: string): TaskDefinition` — reads `<repoPath>/.queohoh/tasks/<taskName>/{config.yaml,prompt.md}`.
  - `listDefinitions(repoPath: string, repoName: string): TaskDefinition[]` — scans `<repoPath>/.queohoh/tasks/*/`; missing dir → `[]`.

- [ ] **Step 1: Write the failing duration test**

`packages/core/src/__tests__/duration.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { parseDuration } from "../duration.js";

describe("parseDuration", () => {
	it("parses s/m/h/d", () => {
		expect(parseDuration("45s")).toBe(45_000);
		expect(parseDuration("30m")).toBe(1_800_000);
		expect(parseDuration("2h")).toBe(7_200_000);
		expect(parseDuration("7d")).toBe(604_800_000);
	});

	it("rejects garbage", () => {
		for (const bad of ["", "30", "m30", "30x", "-5m"]) {
			expect(() => parseDuration(bad)).toThrow(`invalid duration: ${bad}`);
		}
	});
});
```

- [ ] **Step 2: Run duration test to verify it fails**

Run: `pnpm -F @queohoh/core test -- duration`
Expected: FAIL — cannot find module `../duration.js`.

- [ ] **Step 3: Implement duration**

`packages/core/src/duration.ts`:

```ts
const UNIT_MS: Record<string, number> = {
	s: 1_000,
	m: 60_000,
	h: 3_600_000,
	d: 86_400_000,
};

export function parseDuration(text: string): number {
	const match = /^(\d+)([smhd])$/.exec(text);
	if (!match) throw new Error(`invalid duration: ${text}`);
	const amount = Number(match[1]);
	const unit = UNIT_MS[match[2] as string];
	if (unit === undefined) throw new Error(`invalid duration: ${text}`);
	return amount * unit;
}
```

- [ ] **Step 4: Run duration test to verify it passes**

Run: `pnpm -F @queohoh/core test -- duration`
Expected: PASS (2 tests).

- [ ] **Step 5: Write the failing definition test**

`packages/core/src/__tests__/definition.test.ts`:

```ts
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { listDefinitions, loadDefinition } from "../definition.js";

function makeRepo(defs: Record<string, { config: string; prompt: string }>) {
	const repo = mkdtempSync(join(tmpdir(), "queohoh-repo-"));
	for (const [name, files] of Object.entries(defs)) {
		const dir = join(repo, ".queohoh", "tasks", name);
		mkdirSync(dir, { recursive: true });
		writeFileSync(join(dir, "config.yaml"), files.config);
		writeFileSync(join(dir, "prompt.md"), files.prompt);
	}
	return repo;
}

const PR_REVIEW_CONFIG = `
discovery:
  command: gh pr list --json number,title
  item_key: "{{number}}"
args: [number]
worktree: "pr:{{number}}"
pre_run: mise run setup
model: opus
timeout: 45m
priority: high
`;

describe("loadDefinition", () => {
	it("loads a full definition with defaults applied", () => {
		const repo = makeRepo({
			"pr-review": { config: PR_REVIEW_CONFIG, prompt: "Review PR {{number}}.\n" },
		});
		const def = loadDefinition(repo, "platform", "pr-review");
		expect(def).toEqual({
			name: "pr-review",
			repo: "platform",
			discovery: {
				command: "gh pr list --json number,title",
				itemKey: "{{number}}",
			},
			args: ["number"],
			dedup: "skip_seen",
			worktree: "pr:{{number}}",
			preRun: "mise run setup",
			postRun: null,
			model: "opus",
			timeoutMs: 2_700_000,
			priority: "high",
			prompt: "Review PR {{number}}.\n",
		});
	});

	it("applies defaults for a minimal config", () => {
		const repo = makeRepo({ tidy: { config: "{}", prompt: "Tidy up.\n" } });
		const def = loadDefinition(repo, "platform", "tidy");
		expect(def.dedup).toBe("skip_seen");
		expect(def.worktree).toBe("temp");
		expect(def.model).toBe("sonnet");
		expect(def.timeoutMs).toBe(1_800_000);
		expect(def.priority).toBe("normal");
		expect(def.discovery).toBeNull();
		expect(def.args).toEqual([]);
	});

	it("rejects a bad dedup value", () => {
		const repo = makeRepo({
			bad: { config: "dedup: sometimes", prompt: "x" },
		});
		expect(() => loadDefinition(repo, "platform", "bad")).toThrow();
	});
});

describe("listDefinitions", () => {
	it("lists all definition folders", () => {
		const repo = makeRepo({
			a: { config: "{}", prompt: "a" },
			b: { config: "{}", prompt: "b" },
		});
		expect(listDefinitions(repo, "platform").map((d) => d.name)).toEqual([
			"a",
			"b",
		]);
	});

	it("returns [] when .queohoh/tasks is absent", () => {
		const repo = mkdtempSync(join(tmpdir(), "queohoh-empty-"));
		expect(listDefinitions(repo, "platform")).toEqual([]);
	});
});
```

- [ ] **Step 6: Run definition test to verify it fails**

Run: `pnpm -F @queohoh/core test -- definition`
Expected: FAIL — cannot find module `../definition.js`.

- [ ] **Step 7: Implement definitions**

`packages/core/src/definition.ts`:

```ts
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import yaml from "js-yaml";
import { z } from "zod";
import { parseDuration } from "./duration.js";
import type { Priority } from "./task.js";
import { PrioritySchema } from "./task.js";

const DefinitionConfigSchema = z.object({
	discovery: z
		.object({ command: z.string().min(1), item_key: z.string().min(1) })
		.optional(),
	args: z.array(z.string()).default([]),
	dedup: z.enum(["skip_seen", "retry_errored", "none"]).default("skip_seen"),
	worktree: z.string().default("temp"),
	pre_run: z.string().optional(),
	post_run: z.string().optional(),
	model: z.string().default("sonnet"),
	timeout: z.string().default("30m"),
	priority: PrioritySchema.default("normal"),
});

export interface TaskDefinition {
	name: string;
	repo: string;
	discovery: { command: string; itemKey: string } | null;
	args: string[];
	dedup: "skip_seen" | "retry_errored" | "none";
	worktree: string;
	preRun: string | null;
	postRun: string | null;
	model: string;
	timeoutMs: number;
	priority: Priority;
	prompt: string;
}

function tasksDir(repoPath: string): string {
	return join(repoPath, ".queohoh", "tasks");
}

export function loadDefinition(
	repoPath: string,
	repoName: string,
	taskName: string,
): TaskDefinition {
	const dir = join(tasksDir(repoPath), taskName);
	const raw = yaml.load(readFileSync(join(dir, "config.yaml"), "utf-8")) ?? {};
	const config = DefinitionConfigSchema.parse(raw);
	const prompt = readFileSync(join(dir, "prompt.md"), "utf-8");
	return {
		name: taskName,
		repo: repoName,
		discovery: config.discovery
			? { command: config.discovery.command, itemKey: config.discovery.item_key }
			: null,
		args: config.args,
		dedup: config.dedup,
		worktree: config.worktree,
		preRun: config.pre_run ?? null,
		postRun: config.post_run ?? null,
		model: config.model,
		timeoutMs: parseDuration(config.timeout),
		priority: config.priority,
		prompt,
	};
}

export function listDefinitions(
	repoPath: string,
	repoName: string,
): TaskDefinition[] {
	const dir = tasksDir(repoPath);
	if (!existsSync(dir)) return [];
	return readdirSync(dir, { withFileTypes: true })
		.filter((entry) => entry.isDirectory())
		.map((entry) => loadDefinition(repoPath, repoName, entry.name))
		.sort((a, b) => a.name.localeCompare(b.name));
}
```

- [ ] **Step 8: Run definition test to verify it passes**

Run: `pnpm -F @queohoh/core test -- definition`
Expected: PASS (5 tests).

- [ ] **Step 9: Commit**

```bash
git add packages/core/src/duration.ts packages/core/src/definition.ts packages/core/src/__tests__/duration.test.ts packages/core/src/__tests__/definition.test.ts
git commit -m "feat(core): task definitions with duration parsing"
```

---

### Task 8: Global + repo config

**Files:**
- Create: `packages/core/src/config.ts`
- Test: `packages/core/src/__tests__/config.test.ts`

**Interfaces:**
- Consumes: js-yaml, zod.
- Produces:
  - `interface GlobalConfig { projects: { name: string; path: string }[]; maxConcurrentTasks: number; archiveAfterDays: number; vars: Record<string, string> }`
  - `loadGlobalConfig(path: string): GlobalConfig` — zod-validated; defaults `maxConcurrentTasks: 3`, `archiveAfterDays: 7`, `vars: {}`; project `path` expands leading `~/`; missing file → `Error(\`config not found: ${path}\`)`.
  - `interface RepoConfig { vars: Record<string, string> }`
  - `loadRepoConfig(repoPath: string): RepoConfig` — reads `<repoPath>/.queohoh/config.yaml`; missing file → `{ vars: {} }` (repo config is optional).

- [ ] **Step 1: Write the failing test**

`packages/core/src/__tests__/config.test.ts`:

```ts
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { loadGlobalConfig, loadRepoConfig } from "../config.js";

describe("loadGlobalConfig", () => {
	it("parses projects and applies defaults", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-cfg-"));
		const path = join(dir, "config.yaml");
		writeFileSync(
			path,
			[
				"projects:",
				"  - name: platform",
				"    path: ~/workspace/platform",
				"vars:",
				"  github_user: noootown",
			].join("\n"),
		);
		const config = loadGlobalConfig(path);
		expect(config.projects).toEqual([
			{ name: "platform", path: join(homedir(), "workspace/platform") },
		]);
		expect(config.maxConcurrentTasks).toBe(3);
		expect(config.archiveAfterDays).toBe(7);
		expect(config.vars).toEqual({ github_user: "noootown" });
	});

	it("throws on missing file", () => {
		expect(() => loadGlobalConfig("/nope/config.yaml")).toThrow(
			"config not found: /nope/config.yaml",
		);
	});
});

describe("loadRepoConfig", () => {
	it("reads vars from .queohoh/config.yaml", () => {
		const repo = mkdtempSync(join(tmpdir(), "queohoh-repocfg-"));
		mkdirSync(join(repo, ".queohoh"), { recursive: true });
		writeFileSync(
			join(repo, ".queohoh", "config.yaml"),
			"vars:\n  service: rate-review\n",
		);
		expect(loadRepoConfig(repo)).toEqual({
			vars: { service: "rate-review" },
		});
	});

	it("returns empty config when the file is absent", () => {
		const repo = mkdtempSync(join(tmpdir(), "queohoh-repocfg-"));
		expect(loadRepoConfig(repo)).toEqual({ vars: {} });
	});
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm -F @queohoh/core test -- config`
Expected: FAIL — cannot find module `../config.js`.

- [ ] **Step 3: Implement**

`packages/core/src/config.ts`:

```ts
import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import yaml from "js-yaml";
import { z } from "zod";

const GlobalConfigSchema = z.object({
	projects: z
		.array(z.object({ name: z.string().min(1), path: z.string().min(1) }))
		.default([]),
	max_concurrent_tasks: z.number().int().positive().default(3),
	archive_after_days: z.number().int().positive().default(7),
	vars: z.record(z.string(), z.string()).default({}),
});

export interface GlobalConfig {
	projects: { name: string; path: string }[];
	maxConcurrentTasks: number;
	archiveAfterDays: number;
	vars: Record<string, string>;
}

function expandTilde(path: string): string {
	return path.startsWith("~/") ? join(homedir(), path.slice(2)) : path;
}

export function loadGlobalConfig(path: string): GlobalConfig {
	if (!existsSync(path)) throw new Error(`config not found: ${path}`);
	const raw = yaml.load(readFileSync(path, "utf-8")) ?? {};
	const config = GlobalConfigSchema.parse(raw);
	return {
		projects: config.projects.map((p) => ({
			name: p.name,
			path: expandTilde(p.path),
		})),
		maxConcurrentTasks: config.max_concurrent_tasks,
		archiveAfterDays: config.archive_after_days,
		vars: config.vars,
	};
}

const RepoConfigSchema = z.object({
	vars: z.record(z.string(), z.string()).default({}),
});

export interface RepoConfig {
	vars: Record<string, string>;
}

export function loadRepoConfig(repoPath: string): RepoConfig {
	const path = join(repoPath, ".queohoh", "config.yaml");
	if (!existsSync(path)) return { vars: {} };
	const raw = yaml.load(readFileSync(path, "utf-8")) ?? {};
	return RepoConfigSchema.parse(raw);
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `pnpm -F @queohoh/core test -- config`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add packages/core/src/config.ts packages/core/src/__tests__/config.test.ts
git commit -m "feat(core): global and per-repo config loading"
```

---

### Task 9: Scheduler

**Files:**
- Create: `packages/core/src/scheduler.ts`
- Test: `packages/core/src/__tests__/scheduler.test.ts`

**Interfaces:**
- Consumes: `TaskInstance`, `laneKey` (Task 5).
- Produces:
  - `interface LiveState { runningLanes: Set<string>; interactiveLanes: Set<string>; runningCount: number }` (lanes are laneKey strings)
  - `interface ScheduleDecision { start: TaskInstance[]; resolve: TaskInstance[] }`
  - `schedule(tasks: TaskInstance[], live: LiveState, opts: { maxConcurrent: number }): ScheduleDecision`

  Rules (each is a test):
  1. Only `queued` tasks are eligible (`needs-input`, `running`, `done`, `failed` never scheduled).
  2. Order: priority band (`high`, `normal`, `low`), then id ascending within band.
  3. A resolved task starts only if its lane is not in `runningLanes`, not in `interactiveLanes`, and not paused.
  4. Paused lanes: any lane containing a `failed` task (derived from `tasks`).
  5. Global cap: `start.length + resolve.length + live.runningCount <= opts.maxConcurrent`.
  6. Unresolved tasks (worktree null) go to `resolve` (they occupy a slot; they start on a later tick).
  7. At most one task per lane per decision (two queued tasks in one free lane → only the first starts).

- [ ] **Step 1: Write the failing test**

`packages/core/src/__tests__/scheduler.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { schedule } from "../scheduler.js";
import type { LiveState } from "../scheduler.js";
import type { Priority, TaskInstance, TaskStatus } from "../task.js";

let seq = 0;
function task(overrides: {
	status?: TaskStatus;
	priority?: Priority;
	worktree?: string | null;
	repo?: string;
}): TaskInstance {
	seq += 1;
	return {
		id: `01TEST${String(seq).padStart(20, "0")}`,
		status: overrides.status ?? "queued",
		definition: null,
		item: null,
		itemKey: null,
		target: {
			repo: overrides.repo ?? "platform",
			ref: "temp",
			worktree: overrides.worktree === undefined ? "wt-a" : overrides.worktree,
		},
		priority: overrides.priority ?? "normal",
		created: "2026-07-08T00:00:00.000Z",
		source: "tui",
		ephemeralWorktree: false,
		error: null,
		prompt: "p",
	};
}

const idle: LiveState = {
	runningLanes: new Set(),
	interactiveLanes: new Set(),
	runningCount: 0,
};

describe("schedule", () => {
	it("starts a queued resolved task on a free lane", () => {
		const t = task({});
		expect(schedule([t], idle, { maxConcurrent: 3 })).toEqual({
			start: [t],
			resolve: [],
		});
	});

	it("ignores non-queued statuses", () => {
		const tasks = (["needs-input", "running", "done"] as const).map((status) =>
			task({ status, worktree: `wt-${status}` }),
		);
		expect(schedule(tasks, idle, { maxConcurrent: 5 }).start).toEqual([]);
	});

	it("orders by priority band then id", () => {
		const low = task({ priority: "low", worktree: "wt-1" });
		const high = task({ priority: "high", worktree: "wt-2" });
		const normal = task({ priority: "normal", worktree: "wt-3" });
		const { start } = schedule([low, high, normal], idle, { maxConcurrent: 3 });
		expect(start.map((t) => t.id)).toEqual([high.id, normal.id, low.id]);
	});

	it("skips lanes that are running or interactive", () => {
		const a = task({ worktree: "busy" });
		const b = task({ worktree: "yours" });
		const live: LiveState = {
			runningLanes: new Set(["platform:busy"]),
			interactiveLanes: new Set(["platform:yours"]),
			runningCount: 1,
		};
		expect(schedule([a, b], live, { maxConcurrent: 5 }).start).toEqual([]);
	});

	it("pauses a lane containing a failed task", () => {
		const failed = task({ status: "failed", worktree: "wt-a" });
		const queued = task({ worktree: "wt-a" });
		expect(schedule([failed, queued], idle, { maxConcurrent: 3 }).start).toEqual(
			[],
		);
	});

	it("enforces the global cap across start + resolve + running", () => {
		const a = task({ worktree: "wt-1" });
		const b = task({ worktree: null });
		const c = task({ worktree: "wt-3" });
		const live: LiveState = { ...idle, runningCount: 1 };
		const decision = schedule([a, b, c], live, { maxConcurrent: 2 });
		expect(decision.start).toEqual([a]);
		expect(decision.resolve).toEqual([]);
	});

	it("routes unresolved tasks to resolve", () => {
		const t = task({ worktree: null });
		expect(schedule([t], idle, { maxConcurrent: 3 })).toEqual({
			start: [],
			resolve: [t],
		});
	});

	it("starts at most one task per lane per decision", () => {
		const first = task({ worktree: "wt-a" });
		const second = task({ worktree: "wt-a" });
		const { start } = schedule([first, second], idle, { maxConcurrent: 5 });
		expect(start).toEqual([first]);
	});
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm -F @queohoh/core test -- scheduler`
Expected: FAIL — cannot find module `../scheduler.js`.

- [ ] **Step 3: Implement**

`packages/core/src/scheduler.ts`:

```ts
import type { TaskInstance } from "./task.js";
import { laneKey } from "./task.js";

export interface LiveState {
	runningLanes: Set<string>;
	interactiveLanes: Set<string>;
	runningCount: number;
}

export interface ScheduleDecision {
	start: TaskInstance[];
	resolve: TaskInstance[];
}

const PRIORITY_ORDER = { high: 0, normal: 1, low: 2 } as const;

export function schedule(
	tasks: TaskInstance[],
	live: LiveState,
	opts: { maxConcurrent: number },
): ScheduleDecision {
	const pausedLanes = new Set<string>();
	for (const t of tasks) {
		if (t.status === "failed") {
			const lane = laneKey(t);
			if (lane) pausedLanes.add(lane);
		}
	}

	const eligible = tasks
		.filter((t) => t.status === "queued")
		.sort((a, b) => {
			const band = PRIORITY_ORDER[a.priority] - PRIORITY_ORDER[b.priority];
			return band !== 0 ? band : a.id.localeCompare(b.id);
		});

	const start: TaskInstance[] = [];
	const resolve: TaskInstance[] = [];
	const claimedLanes = new Set<string>();
	let slots = opts.maxConcurrent - live.runningCount;

	for (const t of eligible) {
		if (slots <= 0) break;
		const lane = laneKey(t);
		if (lane === null) {
			resolve.push(t);
			slots -= 1;
			continue;
		}
		if (
			live.runningLanes.has(lane) ||
			live.interactiveLanes.has(lane) ||
			pausedLanes.has(lane) ||
			claimedLanes.has(lane)
		) {
			continue;
		}
		start.push(t);
		claimedLanes.add(lane);
		slots -= 1;
	}

	return { start, resolve };
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `pnpm -F @queohoh/core test -- scheduler`
Expected: PASS (8 tests).

- [ ] **Step 5: Commit**

```bash
git add packages/core/src/scheduler.ts packages/core/src/__tests__/scheduler.test.ts
git commit -m "feat(core): pure lane-based scheduler"
```

---

### Task 10: Worktree resolver

**Files:**
- Create: `packages/core/src/resolver.ts`
- Test: `packages/core/src/__tests__/resolver.test.ts`

**Interfaces:**
- Consumes: `TargetRef`, `parseRef`, `extractTicketId` (Task 4).
- Produces:
  - `interface WorktreeInfo { name: string; path: string; branch: string }`
  - `interface ResolverIO { listWorktrees(repoPath: string): Promise<WorktreeInfo[]>; prBranch(repoPath: string, number: number): Promise<string | null>; spawnWorktree(repoPath: string, name: string, branch?: string): Promise<WorktreeInfo> }`
  - `type Resolution = { outcome: "resolved"; worktree: string; ephemeral: boolean } | { outcome: "needs-input"; reason: string }`
  - `resolveTarget(rawRef: string, ctx: { repoPath: string; tempName?: () => string }, io: ResolverIO): Promise<Resolution>` — implements the spec chain:
    1. `worktree:<name>` → use if exists, else needs-input.
    2. `pr:<N>` → branch via `io.prBranch`; PR not found → needs-input; worktree on that branch → use; else ticket id from branch name → worktree named `<ticket>` exists → use : spawn(`<ticket>`, branch); no ticket in branch → needs-input.
    3. `ticket:<ID>` → worktree named `<ID>` exists → use : spawn(`<ID>`).
    4. `temp` → spawn `ctx.tempName()` (default: `tmp-<ulid-last-6-lowercase>`), `ephemeral: true`.
    5. Unparseable ref → needs-input (never throws).

- [ ] **Step 1: Write the failing test**

`packages/core/src/__tests__/resolver.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { resolveTarget } from "../resolver.js";
import type { ResolverIO, WorktreeInfo } from "../resolver.js";

function stubIO(overrides: Partial<ResolverIO> = {}): ResolverIO & {
	spawned: { name: string; branch?: string }[];
} {
	const spawned: { name: string; branch?: string }[] = [];
	return {
		spawned,
		listWorktrees: async () => [],
		prBranch: async () => null,
		spawnWorktree: async (_repo, name, branch) => {
			spawned.push({ name, branch });
			return { name, path: `/wt/${name}`, branch: branch ?? name };
		},
		...overrides,
	};
}

const wt = (name: string, branch = name): WorktreeInfo => ({
	name,
	path: `/wt/${name}`,
	branch,
});

const ctx = { repoPath: "/repo", tempName: () => "tmp-fix-abc123" };

describe("resolveTarget", () => {
	it("worktree ref: uses existing", async () => {
		const io = stubIO({ listWorktrees: async () => [wt("main")] });
		expect(await resolveTarget("worktree:main", ctx, io)).toEqual({
			outcome: "resolved",
			worktree: "main",
			ephemeral: false,
		});
	});

	it("worktree ref: needs-input when absent", async () => {
		const result = await resolveTarget("worktree:gone", ctx, stubIO());
		expect(result.outcome).toBe("needs-input");
	});

	it("pr ref: matches existing worktree by branch", async () => {
		const io = stubIO({
			prBranch: async () => "JUS-1423-fix-auth",
			listWorktrees: async () => [wt("anything", "JUS-1423-fix-auth")],
		});
		expect(await resolveTarget("pr:1423", ctx, io)).toEqual({
			outcome: "resolved",
			worktree: "anything",
			ephemeral: false,
		});
	});

	it("pr ref: spawns ticket-named worktree from branch ticket id", async () => {
		const io = stubIO({ prBranch: async () => "JUS-1423-fix-auth" });
		expect(await resolveTarget("pr:1423", ctx, io)).toEqual({
			outcome: "resolved",
			worktree: "JUS-1423",
			ephemeral: false,
		});
		expect(io.spawned).toEqual([
			{ name: "JUS-1423", branch: "JUS-1423-fix-auth" },
		]);
	});

	it("pr ref: needs-input when pr not found", async () => {
		const result = await resolveTarget("pr:9999", ctx, stubIO());
		expect(result.outcome).toBe("needs-input");
	});

	it("pr ref: needs-input when branch has no ticket id", async () => {
		const io = stubIO({ prBranch: async () => "random-branch-name" });
		const result = await resolveTarget("pr:1423", ctx, io);
		expect(result.outcome).toBe("needs-input");
	});

	it("ticket ref: uses existing worktree named by ticket", async () => {
		const io = stubIO({ listWorktrees: async () => [wt("JUS-77")] });
		expect(await resolveTarget("ticket:JUS-77", ctx, io)).toEqual({
			outcome: "resolved",
			worktree: "JUS-77",
			ephemeral: false,
		});
		expect(io.spawned).toEqual([]);
	});

	it("ticket ref: spawns when absent", async () => {
		const io = stubIO();
		expect(await resolveTarget("ticket:JUS-77", ctx, io)).toEqual({
			outcome: "resolved",
			worktree: "JUS-77",
			ephemeral: false,
		});
		expect(io.spawned).toEqual([{ name: "JUS-77", branch: undefined }]);
	});

	it("temp ref: spawns ephemeral with generated name", async () => {
		const io = stubIO();
		expect(await resolveTarget("temp", ctx, io)).toEqual({
			outcome: "resolved",
			worktree: "tmp-fix-abc123",
			ephemeral: true,
		});
	});

	it("garbage ref: needs-input, never throws", async () => {
		const result = await resolveTarget("wat:?", ctx, stubIO());
		expect(result.outcome).toBe("needs-input");
	});
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm -F @queohoh/core test -- resolver`
Expected: FAIL — cannot find module `../resolver.js`.

- [ ] **Step 3: Implement**

`packages/core/src/resolver.ts`:

```ts
import { ulid } from "ulid";
import { extractTicketId, parseRef } from "./ref.js";
import type { TargetRef } from "./ref.js";

export interface WorktreeInfo {
	name: string;
	path: string;
	branch: string;
}

export interface ResolverIO {
	listWorktrees(repoPath: string): Promise<WorktreeInfo[]>;
	prBranch(repoPath: string, number: number): Promise<string | null>;
	spawnWorktree(
		repoPath: string,
		name: string,
		branch?: string,
	): Promise<WorktreeInfo>;
}

export type Resolution =
	| { outcome: "resolved"; worktree: string; ephemeral: boolean }
	| { outcome: "needs-input"; reason: string };

function defaultTempName(): string {
	return `tmp-${ulid().slice(-6).toLowerCase()}`;
}

export async function resolveTarget(
	rawRef: string,
	ctx: { repoPath: string; tempName?: () => string },
	io: ResolverIO,
): Promise<Resolution> {
	let ref: TargetRef;
	try {
		ref = parseRef(rawRef);
	} catch {
		return { outcome: "needs-input", reason: `unrecognized ref: ${rawRef}` };
	}

	switch (ref.kind) {
		case "worktree": {
			const existing = await io.listWorktrees(ctx.repoPath);
			const match = existing.find((w) => w.name === ref.name);
			if (match) {
				return { outcome: "resolved", worktree: match.name, ephemeral: false };
			}
			return {
				outcome: "needs-input",
				reason: `worktree not found: ${ref.name}`,
			};
		}
		case "pr": {
			const branch = await io.prBranch(ctx.repoPath, ref.number);
			if (branch === null) {
				return {
					outcome: "needs-input",
					reason: `PR not found: #${ref.number}`,
				};
			}
			const existing = await io.listWorktrees(ctx.repoPath);
			const byBranch = existing.find((w) => w.branch === branch);
			if (byBranch) {
				return {
					outcome: "resolved",
					worktree: byBranch.name,
					ephemeral: false,
				};
			}
			const ticket = extractTicketId(branch);
			if (ticket === null) {
				return {
					outcome: "needs-input",
					reason: `no ticket id in branch: ${branch}`,
				};
			}
			const byName = existing.find((w) => w.name === ticket);
			if (byName) {
				return { outcome: "resolved", worktree: byName.name, ephemeral: false };
			}
			const spawned = await io.spawnWorktree(ctx.repoPath, ticket, branch);
			return { outcome: "resolved", worktree: spawned.name, ephemeral: false };
		}
		case "ticket": {
			const existing = await io.listWorktrees(ctx.repoPath);
			const match = existing.find((w) => w.name === ref.id);
			if (match) {
				return { outcome: "resolved", worktree: match.name, ephemeral: false };
			}
			const spawned = await io.spawnWorktree(ctx.repoPath, ref.id);
			return { outcome: "resolved", worktree: spawned.name, ephemeral: false };
		}
		case "temp": {
			const name = (ctx.tempName ?? defaultTempName)();
			const spawned = await io.spawnWorktree(ctx.repoPath, name);
			return { outcome: "resolved", worktree: spawned.name, ephemeral: true };
		}
	}
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `pnpm -F @queohoh/core test -- resolver`
Expected: PASS (10 tests).

- [ ] **Step 5: Commit**

```bash
git add packages/core/src/resolver.ts packages/core/src/__tests__/resolver.test.ts
git commit -m "feat(core): deterministic worktree resolver"
```

---

### Task 11: Resolver IO (git/gh/wt adapters)

**Files:**
- Create: `packages/core/src/resolver-io.ts`
- Test: `packages/core/src/__tests__/resolver-io.test.ts`

**Interfaces:**
- Consumes: `ResolverIO`, `WorktreeInfo` (Task 10).
- Produces:
  - `type Exec = (command: string, args: string[], opts: { cwd: string }) => Promise<{ stdout: string; exitCode: number }>`
  - `parseWorktreePorcelain(output: string): WorktreeInfo[]` — pure parser for `git worktree list --porcelain` (name = basename of path; detached/bare entries skipped).
  - `createResolverIO(exec: Exec): ResolverIO` where:
    - `listWorktrees` runs `git worktree list --porcelain`.
    - `prBranch` runs `gh pr view <n> --json headRefName` and returns `headRefName`, or null on nonzero exit.
    - `spawnWorktree` runs `wt switch -c <branch-or-name>` in the repo, then re-lists to find and return the new worktree; throws if it can't be found after spawn.
  - `defaultExec: Exec` — `child_process.execFile` wrapper, never rejects on nonzero exit (returns exitCode).

- [ ] **Step 1: Write the failing test**

`packages/core/src/__tests__/resolver-io.test.ts` (all tests use injected fake exec — no real git/gh/wt):

```ts
import { describe, expect, it } from "vitest";
import { createResolverIO, parseWorktreePorcelain } from "../resolver-io.js";
import type { Exec } from "../resolver-io.js";

const PORCELAIN = [
	"worktree /Users/me/ws/platform",
	"HEAD abc123",
	"branch refs/heads/main",
	"",
	"worktree /Users/me/ws/platform-worktrees/JUS-1423",
	"HEAD def456",
	"branch refs/heads/JUS-1423-fix-auth",
	"",
	"worktree /Users/me/ws/platform-worktrees/detached",
	"HEAD 999999",
	"detached",
	"",
].join("\n");

describe("parseWorktreePorcelain", () => {
	it("parses name/path/branch and skips detached", () => {
		expect(parseWorktreePorcelain(PORCELAIN)).toEqual([
			{ name: "platform", path: "/Users/me/ws/platform", branch: "main" },
			{
				name: "JUS-1423",
				path: "/Users/me/ws/platform-worktrees/JUS-1423",
				branch: "JUS-1423-fix-auth",
			},
		]);
	});

	it("returns [] for empty output", () => {
		expect(parseWorktreePorcelain("")).toEqual([]);
	});
});

function fakeExec(
	responses: Record<string, { stdout: string; exitCode: number }>,
): Exec & { calls: string[] } {
	const calls: string[] = [];
	const fn = (async (command, args) => {
		const key = [command, ...args].join(" ");
		calls.push(key);
		return responses[key] ?? { stdout: "", exitCode: 1 };
	}) as Exec & { calls: string[] };
	fn.calls = calls;
	return fn;
}

describe("createResolverIO", () => {
	it("listWorktrees shells to git", async () => {
		const exec = fakeExec({
			"git worktree list --porcelain": { stdout: PORCELAIN, exitCode: 0 },
		});
		const io = createResolverIO(exec);
		const list = await io.listWorktrees("/repo");
		expect(list.map((w) => w.name)).toEqual(["platform", "JUS-1423"]);
	});

	it("prBranch returns headRefName on success, null on failure", async () => {
		const exec = fakeExec({
			"gh pr view 1423 --json headRefName": {
				stdout: '{"headRefName":"JUS-1423-fix-auth"}',
				exitCode: 0,
			},
		});
		const io = createResolverIO(exec);
		expect(await io.prBranch("/repo", 1423)).toBe("JUS-1423-fix-auth");
		expect(await io.prBranch("/repo", 9999)).toBeNull();
	});

	it("spawnWorktree runs wt then finds the new worktree", async () => {
		const before = PORCELAIN;
		const after = `${PORCELAIN}worktree /Users/me/ws/platform-worktrees/JUS-77\nHEAD aaa\nbranch refs/heads/JUS-77\n\n`;
		let listCalls = 0;
		const exec: Exec = async (command, args) => {
			const key = [command, ...args].join(" ");
			if (key === "git worktree list --porcelain") {
				listCalls += 1;
				return { stdout: listCalls > 1 ? after : before, exitCode: 0 };
			}
			if (key === "wt switch -c JUS-77") return { stdout: "", exitCode: 0 };
			return { stdout: "", exitCode: 1 };
		};
		const io = createResolverIO(exec);
		const spawned = await io.spawnWorktree("/repo", "JUS-77");
		expect(spawned.name).toBe("JUS-77");
	});

	it("spawnWorktree throws when wt fails", async () => {
		const exec = fakeExec({
			"git worktree list --porcelain": { stdout: PORCELAIN, exitCode: 0 },
		});
		const io = createResolverIO(exec);
		await expect(io.spawnWorktree("/repo", "JUS-77")).rejects.toThrow(
			/failed to spawn worktree/,
		);
	});
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm -F @queohoh/core test -- resolver-io`
Expected: FAIL — cannot find module `../resolver-io.js`.

- [ ] **Step 3: Implement**

`packages/core/src/resolver-io.ts`:

```ts
import { execFile } from "node:child_process";
import { basename } from "node:path";
import type { ResolverIO, WorktreeInfo } from "./resolver.js";

export type Exec = (
	command: string,
	args: string[],
	opts: { cwd: string },
) => Promise<{ stdout: string; exitCode: number }>;

export const defaultExec: Exec = (command, args, opts) =>
	new Promise((resolve) => {
		execFile(command, args, { cwd: opts.cwd }, (error, stdout) => {
			const exitCode =
				error && typeof error.code === "number" ? error.code : error ? 1 : 0;
			resolve({ stdout: stdout ?? "", exitCode });
		});
	});

export function parseWorktreePorcelain(output: string): WorktreeInfo[] {
	const result: WorktreeInfo[] = [];
	for (const block of output.split("\n\n")) {
		const lines = block.split("\n").filter(Boolean);
		const pathLine = lines.find((l) => l.startsWith("worktree "));
		const branchLine = lines.find((l) => l.startsWith("branch "));
		if (!pathLine || !branchLine) continue; // detached / bare / junk
		const path = pathLine.slice("worktree ".length);
		const branch = branchLine
			.slice("branch ".length)
			.replace(/^refs\/heads\//, "");
		result.push({ name: basename(path), path, branch });
	}
	return result;
}

export function createResolverIO(exec: Exec): ResolverIO {
	async function listWorktrees(repoPath: string): Promise<WorktreeInfo[]> {
		const { stdout, exitCode } = await exec(
			"git",
			["worktree", "list", "--porcelain"],
			{ cwd: repoPath },
		);
		if (exitCode !== 0) return [];
		return parseWorktreePorcelain(stdout);
	}

	return {
		listWorktrees,

		async prBranch(repoPath, number) {
			const { stdout, exitCode } = await exec(
				"gh",
				["pr", "view", String(number), "--json", "headRefName"],
				{ cwd: repoPath },
			);
			if (exitCode !== 0) return null;
			try {
				const parsed = JSON.parse(stdout) as { headRefName?: string };
				return parsed.headRefName ?? null;
			} catch {
				return null;
			}
		},

		async spawnWorktree(repoPath, name, branch) {
			const { exitCode } = await exec("wt", ["switch", "-c", branch ?? name], {
				cwd: repoPath,
			});
			if (exitCode === 0) {
				const after = await listWorktrees(repoPath);
				const spawned =
					after.find((w) => w.name === name) ??
					after.find((w) => w.branch === (branch ?? name));
				if (spawned) return spawned;
			}
			throw new Error(`failed to spawn worktree: ${name}`);
		},
	};
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `pnpm -F @queohoh/core test -- resolver-io`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add packages/core/src/resolver-io.ts packages/core/src/__tests__/resolver-io.test.ts
git commit -m "feat(core): resolver IO adapters for git/gh/wt"
```

---

### Task 12: Public API barrel + full suite

**Files:**
- Modify: `packages/core/src/index.ts`
- Test: existing suite.

**Interfaces:**
- Consumes: all previous tasks.
- Produces: `@queohoh/core` exports everything Plans B–D consume: `render`, `parseRef`, `formatRef`, `extractTicketId`, task types + `parseTaskFile`/`serializeTaskFile`/`laneKey`, `QueueStore`/`NewTaskInput`, `parseDuration`, `loadDefinition`/`listDefinitions`/`TaskDefinition`, `loadGlobalConfig`/`loadRepoConfig`, `schedule`/`LiveState`/`ScheduleDecision`, `resolveTarget`/`ResolverIO`/`Resolution`/`WorktreeInfo`, `createResolverIO`/`defaultExec`/`parseWorktreePorcelain`.

- [ ] **Step 1: Write the barrel**

`packages/core/src/index.ts`:

```ts
export { render } from "./template.js";
export { extractTicketId, formatRef, parseRef } from "./ref.js";
export type { TargetRef } from "./ref.js";
export {
	laneKey,
	parseTaskFile,
	PrioritySchema,
	serializeTaskFile,
	TaskSourceSchema,
	TaskStatusSchema,
} from "./task.js";
export type { Priority, TaskInstance, TaskSource, TaskStatus } from "./task.js";
export { QueueStore } from "./store.js";
export type { NewTaskInput } from "./store.js";
export { parseDuration } from "./duration.js";
export { listDefinitions, loadDefinition } from "./definition.js";
export type { TaskDefinition } from "./definition.js";
export { loadGlobalConfig, loadRepoConfig } from "./config.js";
export type { GlobalConfig, RepoConfig } from "./config.js";
export { schedule } from "./scheduler.js";
export type { LiveState, ScheduleDecision } from "./scheduler.js";
export { resolveTarget } from "./resolver.js";
export type { Resolution, ResolverIO, WorktreeInfo } from "./resolver.js";
export {
	createResolverIO,
	defaultExec,
	parseWorktreePorcelain,
} from "./resolver-io.js";
export type { Exec } from "./resolver-io.js";
export { parseFrontmatter, stringifyFrontmatter } from "./frontmatter.js";
```

Also delete the now-obsolete `CORE_VERSION` export and update `packages/core/src/__tests__/smoke.test.ts` to:

```ts
import { describe, expect, it } from "vitest";
import { QueueStore, render, schedule } from "../index.js";

describe("public API", () => {
	it("exports the core surface", () => {
		expect(typeof render).toBe("function");
		expect(typeof schedule).toBe("function");
		expect(typeof QueueStore).toBe("function");
	});
});
```

- [ ] **Step 2: Run the full suite**

Run: `pnpm -F @queohoh/core test && pnpm -F @queohoh/core typecheck && pnpm lint:ci`
Expected: all tests PASS (~50), typecheck clean, lint clean.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "feat(core): public API barrel for @queohoh/core"
```

---

## Self-Review Notes

- **Spec coverage (Plan A scope):** monorepo scaffold (T1), task file format + statuses (T5), central store with atomic writes + archive dir (T6), definitions incl. args/dedup/hooks/model/timeout/priority (T7), global + repo config incl. `max_concurrent_tasks` and vars (T8), scheduler rules incl. lane pause + interactive-awareness + global cap (T9), resolver chain incl. ticket convention + temp/ephemeral + needs-input (T10), real git/gh/wt adapters (T11), template vars with precedence (T3). Deliberately deferred to Plan B: discovery/dedup *execution*, hook execution, prompt rendering into runs, runner, daemon loop, redaction, reports. Deferred to Plan C/D: API, MCP, /qoo, TUI.
- **Type consistency:** `TaskInstance` fields match between task.ts, store.ts, scheduler tests; `WorktreeInfo`/`ResolverIO` shared between resolver.ts and resolver-io.ts via import.
- **Placeholder scan:** clean — every step has full code and exact commands.
