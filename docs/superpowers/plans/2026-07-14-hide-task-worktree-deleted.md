# Archive Tasks Whose Worktree Was Deleted — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When a terminal task's spawned worktree no longer exists on disk, archive the task so it leaves the live queue (recoverable via the archived list).

**Architecture:** Add a second archive trigger to the engine's `pass()` tick, next to the existing 7-day age sweep. It reads the already-maintained `worktreeCache` (refreshed each tick) to detect a deleted worktree, and a new `worktreeListingOk` set to avoid false-archiving on a cold/failed cache. Reuses `store.archive` — no data-model, config, API, or TUI change.

**Tech Stack:** TypeScript, Node, Vitest. Package `@queohoh/daemon` (`packages/daemon`), consuming `@queohoh/core`.

## Global Constraints

- Reuse the existing archive machinery (`store.archive`) — do NOT invent a new "hidden" state.
- Terminal statuses are exactly: `done`, `failed`, `skipped`, `cancelled`, `verify-failed` (mirrors the dismiss list at `packages/daemon/src/api.ts:602`).
- `REPO_SENTINEL` is `"@repo"` (`packages/core/src/resolver.ts:11`); primary-checkout tasks carry it and must never be archived by this trigger.
- Do NOT add an `onChange` call in the new sweep — the age sweep it sits beside does not, and every tick already broadcasts a fresh snapshot (`packages/daemon/src/daemon.ts:112`).
- Do NOT change the existing 7-day age sweep behavior.

---

## File Structure

- `packages/daemon/src/engine.ts` — add a module-level `TERMINAL_STATUSES` set, a `worktreeListingOk` field, populate it in `refreshWorktreeCache`'s success branch, and add the worktree-deletion sweep in `pass()`.
- `packages/daemon/src/__tests__/engine.test.ts` — add a `describe("worktree-deletion archive", …)` block.

### Reference: current code shape

`pass()` (`engine.ts:278`) runs, in order: `registry.sweep()`, `evaluateCrons()`, `await refreshWorktreeCache()`, fire-and-forget `refreshGitEnrichment()`, the orphan sweep, the age sweep (`:300–308`), then `buildLiveState`/`schedule`. The new sweep goes immediately after the age sweep.

The age sweep for reference:
```ts
const cutoff = Date.now() - deps.config.archiveAfterDays * 86_400_000;
for (const t of deps.store.list()) {
	if (
		(t.status === "done" || t.status === "cancelled") &&
		Date.parse(t.created) < cutoff
	) {
		deps.store.archive(t.id);
	}
}
```

`refreshWorktreeCache()` (`engine.ts:434`):
```ts
private async refreshWorktreeCache(): Promise<void> {
	for (const project of this.deps.config.projects) {
		try {
			this.worktreeCache.set(
				project.name,
				await this.deps.resolverIO.listWorktrees(project.path),
			);
		} catch {
			if (!this.worktreeCache.has(project.name)) {
				this.worktreeCache.set(project.name, []);
			}
		}
	}
}
```

Test setup helper `setup(overrides)` (`engine.test.ts:32`) builds an `Engine` over a real `QueueStore` in a temp dir. Its default `resolverIO.listWorktrees` returns `[{ name: "JUS-1", path, branch: "JUS-1" }]`. `store.create({ prompt, repo, ref, source })` makes a `queued` task with `target.worktree = null`; `store.update(id, patch)` shallow-merges (pass a full `target` object to replace it). `store.list()` is the live list; `store.listArchived()` is the archived list.

---

## Task 1: Worktree-deletion archive sweep

Adds the sweep in its naive form (reads `worktreeCache` directly). The cold-cache guard is Task 2.

**Files:**
- Modify: `packages/daemon/src/engine.ts` (add `TERMINAL_STATUSES` const near the other top-level consts around `:37`; add sweep in `pass()` after the age sweep, currently ending `:308`)
- Test: `packages/daemon/src/__tests__/engine.test.ts`

**Interfaces:**
- Consumes: `this.worktreeCache: Map<string, WorktreeInfo[]>` (repo name → worktrees), `deps.store.list()`, `deps.store.archive(id)`, `REPO_SENTINEL`, `TaskInstance["status"]`.
- Produces: `const TERMINAL_STATUSES: ReadonlySet<TaskInstance["status"]>` (module-level in `engine.ts`), used again in Task 2.

- [ ] **Step 1: Write the failing tests**

Append to `packages/daemon/src/__tests__/engine.test.ts` (inside the file, after the existing `describe("Engine.tick", …)` block):

```ts
describe("worktree-deletion archive", () => {
	it("archives a terminal task whose worktree was deleted", async () => {
		// Worktree "JUS-1" is gone from the listing.
		const { engine, store } = setup({
			resolverIO: { listWorktrees: async () => [] },
		});
		const t = store.create({
			prompt: "p",
			repo: "platform",
			ref: "worktree:JUS-1",
			source: "tui",
		});
		store.update(t.id, {
			status: "failed",
			target: { repo: "platform", ref: "worktree:JUS-1", worktree: "JUS-1" },
		});

		await engine.tick();

		expect(store.list()).toHaveLength(0);
		expect(store.listArchived().map((a) => a.id)).toContain(t.id);
	});

	it("keeps a terminal task whose worktree still exists", async () => {
		// Default listWorktrees returns [{ name: "JUS-1", … }] — worktree present.
		const { engine, store } = setup();
		const t = store.create({
			prompt: "p",
			repo: "platform",
			ref: "worktree:JUS-1",
			source: "tui",
		});
		store.update(t.id, {
			status: "failed",
			target: { repo: "platform", ref: "worktree:JUS-1", worktree: "JUS-1" },
		});

		await engine.tick();

		expect(store.list().map((x) => x.id)).toContain(t.id);
		expect(store.listArchived()).toHaveLength(0);
	});

	it("does not archive a non-terminal task with a deleted worktree", async () => {
		const { engine, store } = setup({
			resolverIO: { listWorktrees: async () => [] },
		});
		const t = store.create({
			prompt: "p",
			repo: "platform",
			ref: "worktree:JUS-1",
			source: "tui",
		});
		store.update(t.id, {
			status: "needs-input",
			target: { repo: "platform", ref: "worktree:JUS-1", worktree: "JUS-1" },
		});

		await engine.tick();

		expect(store.list()[0]?.status).toBe("needs-input");
		expect(store.listArchived()).toHaveLength(0);
	});

	it("does not archive @repo or null-worktree terminal tasks", async () => {
		const { engine, store } = setup({
			resolverIO: { listWorktrees: async () => [] },
		});
		const sentinel = store.create({
			prompt: "p",
			repo: "platform",
			ref: "repo",
			source: "tui",
		});
		store.update(sentinel.id, {
			status: "failed",
			target: { repo: "platform", ref: "repo", worktree: "@repo" },
		});
		const noWt = store.create({
			prompt: "p",
			repo: "platform",
			ref: "worktree:X",
			source: "tui",
		});
		store.update(noWt.id, {
			status: "failed",
			target: { repo: "platform", ref: "worktree:X", worktree: null },
		});

		await engine.tick();

		expect(store.list().map((x) => x.id).sort()).toEqual(
			[sentinel.id, noWt.id].sort(),
		);
		expect(store.listArchived()).toHaveLength(0);
	});

	it("archives every terminal status on worktree deletion", async () => {
		const statuses = [
			"done",
			"failed",
			"skipped",
			"cancelled",
			"verify-failed",
		] as const;
		for (const status of statuses) {
			const { engine, store } = setup({
				resolverIO: { listWorktrees: async () => [] },
			});
			const t = store.create({
				prompt: "p",
				repo: "platform",
				ref: "worktree:JUS-1",
				source: "tui",
			});
			store.update(t.id, {
				status,
				target: { repo: "platform", ref: "worktree:JUS-1", worktree: "JUS-1" },
			});

			await engine.tick();

			expect(store.list(), `status ${status} should be archived`).toHaveLength(
				0,
			);
			expect(store.listArchived().map((a) => a.id)).toContain(t.id);
		}
	});
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd packages/daemon && pnpm vitest run src/__tests__/engine.test.ts -t "worktree-deletion archive"`
Expected: FAIL — the "archives …" / "archives every terminal status" cases fail (task still in `store.list()`, `listArchived()` empty) because no sweep archives it yet.

- [ ] **Step 3: Add the `TERMINAL_STATUSES` constant**

In `packages/daemon/src/engine.ts`, add near the other top-level declarations (e.g. just below the `GitEnrichment`/`GitCommitFacts` type block around `:37`, before the `Engine` class):

```ts
/**
 * Terminal statuses — a task in one of these will never run again. Mirrors the
 * dismiss list in api.ts. Used by the worktree-deletion archive sweep.
 */
const TERMINAL_STATUSES: ReadonlySet<TaskInstance["status"]> = new Set([
	"done",
	"failed",
	"skipped",
	"cancelled",
	"verify-failed",
]);
```

(`TaskInstance` is already imported at `engine.ts:12`.)

- [ ] **Step 4: Add the sweep in `pass()`**

In `packages/daemon/src/engine.ts`, immediately after the age-sweep loop (the block ending at `:308`, right before `const tasks = deps.store.list();`), insert:

```ts
		// Archive terminal tasks whose spawned worktree has been deleted. Deleting
		// a worktree is a deliberate act (only the removeWorktree RPC), so it reads
		// as "I'm done with this" and outranks the age sweep's "keep failed
		// visible" — this catches the failed/skipped set the age timer never sweeps.
		for (const t of deps.store.list()) {
			const wt = t.target.worktree;
			if (
				!TERMINAL_STATUSES.has(t.status) ||
				wt === null ||
				wt === REPO_SENTINEL
			) {
				continue;
			}
			const known = this.worktreeCache.get(t.target.repo) ?? [];
			if (!known.some((w) => w.name === wt)) {
				deps.store.archive(t.id);
			}
		}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cd packages/daemon && pnpm vitest run src/__tests__/engine.test.ts -t "worktree-deletion archive"`
Expected: PASS — all five cases green.

- [ ] **Step 6: Run the full daemon suite to check for regressions**

Run: `cd packages/daemon && pnpm vitest run`
Expected: PASS — existing engine/age-sweep tests still green.

- [ ] **Step 7: Commit**

```bash
git add packages/daemon/src/engine.ts packages/daemon/src/__tests__/engine.test.ts
git commit -m "feat(daemon): archive terminal tasks whose worktree was deleted"
```

---

## Task 2: Cold-cache guard

`refreshWorktreeCache` seeds a repo's cache with `[]` when its very first `listWorktrees` fails (`engine.ts:447–448`). That seeded `[]` is indistinguishable from a genuine "listed successfully, zero worktrees" — so Task 1's sweep would false-archive every worktree task on a cold start or a never-listable repo. Guard it: only sweep repos whose listing has succeeded at least once.

**Files:**
- Modify: `packages/daemon/src/engine.ts` (add `worktreeListingOk` field near `:103`; populate it in `refreshWorktreeCache`'s success branch; add its check to the sweep condition from Task 1)
- Test: `packages/daemon/src/__tests__/engine.test.ts`

**Interfaces:**
- Consumes: `TERMINAL_STATUSES` (Task 1), `this.worktreeCache` (Task 1).
- Produces: `this.worktreeListingOk: Set<string>` (private field on `Engine`).

- [ ] **Step 1: Write the failing test**

Add inside the `describe("worktree-deletion archive", …)` block in `packages/daemon/src/__tests__/engine.test.ts`:

```ts
	it("does not archive when the repo has never listed successfully", async () => {
		// listWorktrees always throws → refreshWorktreeCache seeds [] for the repo,
		// but the listing never succeeded, so "absent" must NOT count as deleted.
		const { engine, store } = setup({
			resolverIO: {
				listWorktrees: async () => {
					throw new Error("git unavailable");
				},
			},
		});
		const t = store.create({
			prompt: "p",
			repo: "platform",
			ref: "worktree:JUS-1",
			source: "tui",
		});
		store.update(t.id, {
			status: "failed",
			target: { repo: "platform", ref: "worktree:JUS-1", worktree: "JUS-1" },
		});

		await engine.tick();

		expect(store.list().map((x) => x.id)).toContain(t.id);
		expect(store.listArchived()).toHaveLength(0);
	});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd packages/daemon && pnpm vitest run src/__tests__/engine.test.ts -t "never listed successfully"`
Expected: FAIL — Task 1's naive sweep archives the task (seeded `[]` cache has no `JUS-1`), so it is absent from `store.list()` and present in `listArchived()`.

- [ ] **Step 3: Add the `worktreeListingOk` field**

In `packages/daemon/src/engine.ts`, add just below the `worktreeCache` field (`:103`):

```ts
	// Repos whose `listWorktrees` has succeeded at least once this process. Guards
	// the worktree-deletion sweep against a seeded-empty cache (cold start or a
	// never-listable repo), where "worktree absent" would be a false positive.
	private worktreeListingOk = new Set<string>();
```

- [ ] **Step 4: Populate it on a successful listing**

In `refreshWorktreeCache` (`engine.ts:434`), record success inside the `try`, after the `worktreeCache.set`:

```ts
	private async refreshWorktreeCache(): Promise<void> {
		for (const project of this.deps.config.projects) {
			try {
				this.worktreeCache.set(
					project.name,
					await this.deps.resolverIO.listWorktrees(project.path),
				);
				this.worktreeListingOk.add(project.name);
			} catch {
				if (!this.worktreeCache.has(project.name)) {
					this.worktreeCache.set(project.name, []);
				}
			}
		}
	}
```

- [ ] **Step 5: Add the guard to the sweep condition**

In the worktree-deletion sweep added in Task 1, extend the skip condition to also skip repos not yet successfully listed:

```ts
		for (const t of deps.store.list()) {
			const wt = t.target.worktree;
			if (
				!TERMINAL_STATUSES.has(t.status) ||
				wt === null ||
				wt === REPO_SENTINEL ||
				!this.worktreeListingOk.has(t.target.repo)
			) {
				continue;
			}
			const known = this.worktreeCache.get(t.target.repo) ?? [];
			if (!known.some((w) => w.name === wt)) {
				deps.store.archive(t.id);
			}
		}
```

- [ ] **Step 6: Run the new test to verify it passes**

Run: `cd packages/daemon && pnpm vitest run src/__tests__/engine.test.ts -t "never listed successfully"`
Expected: PASS.

- [ ] **Step 7: Run the whole worktree-deletion block + full suite**

Run: `cd packages/daemon && pnpm vitest run src/__tests__/engine.test.ts -t "worktree-deletion archive"`
Expected: PASS — all six cases green (the Task 1 positive cases still archive, because an empty-but-successful listing puts the repo in `worktreeListingOk`).

Run: `cd packages/daemon && pnpm vitest run`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add packages/daemon/src/engine.ts packages/daemon/src/__tests__/engine.test.ts
git commit -m "fix(daemon): guard worktree-deletion archive against cold worktree cache"
```

---

## Self-Review Notes

- **Spec coverage:** design condition 1 (terminal status) → Task 1 `TERMINAL_STATUSES` + "every terminal status" test; condition 2 (real worktree, not null/`@repo`) → Task 1 "does not archive @repo or null-worktree" test; condition 3 (absent from a *confirmed* listing) → Task 1 detection + Task 2 cold-cache guard. Non-goals (no TUI/config/API change, age sweep untouched) → honored; the full-suite run guards against age-sweep regression. Reuse of `store.archive`/recoverability → asserted via `store.listArchived()`.
- **Types:** `TERMINAL_STATUSES` typed `ReadonlySet<TaskInstance["status"]>`, defined in Task 1 and referenced by name in Task 2; `worktreeListingOk: Set<string>` keyed by the same `t.target.repo` value the cache uses (`engine.ts:664` deletes the cache by exactly that key).
- **No placeholders:** every code and command step is concrete.
