# Worktree Protection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make configured worktrees (plus every project's main checkout) impossible to delete through queohoh, surfaced in the TUI with a lock glyph and a gated remove action.

**Architecture:** A shared predicate `isProtectedWorktree(repoPath, protectedNames, wt)` decides protection: a worktree is protected if its path equals the project's registered checkout path (the main checkout) or its name is in the project's `protected_worktrees` list from `vars.yaml`. The daemon enforces it in `Engine.removeWorktree` (authoritative hard block) and emits a `protected` flag on each worktree in the state snapshot. The Rust TUI reads that flag to render 🔒 in the existing marker slot and to gate the remove action (single-select refuses; bulk silently drops protected rows).

**Tech Stack:** TypeScript (`@queohoh/core`, `@queohoh/daemon`, vitest), Rust (`qoo-tui`, ratatui, cargo test / insta).

## Global Constraints

- The protection predicate lives in exactly one place (`isProtectedWorktree` in `packages/core/src/resolver.ts`) and is called by both the enforcement guard and the snapshot enrichment — they must never diverge.
- Main-checkout detection is **path-equality** (`wt.path === repoPath`), never `wt.name === projectName`.
- The `protected_worktrees` loader is **tolerant** (matches `loadProjectModels` / `loadProjectGithubId`): malformed value → `[]`, bad entries skipped, never throws.
- Rust `protected` field uses `#[serde(default)]` → an old daemon that omits it deserializes to `false` (removable affordance; the engine is the real guard).
- Scope is per-project `vars.yaml` only — no global `config.yaml` protected list.
- Spec: `docs/superpowers/specs/2026-07-14-worktree-protection-design.md`.

Test commands (run from repo root unless noted):
- Core targeted: `pnpm --filter @queohoh/core exec vitest run -t "<name>"`
- Daemon targeted: `pnpm --filter @queohoh/daemon exec vitest run -t "<name>"`
- Rust targeted: `cargo test -p qoo-tui <name>`
- Full gate at the end: `mise run check`

---

### Task 1: Core — `loadProjectProtectedWorktrees` loader

**Files:**
- Modify: `packages/core/src/config.ts` (add loader after `loadProjectDefaultModel` at :191; add reserved-skip line in `loadProjectVars` at :131)
- Modify: `packages/core/src/index.ts:4-13` (export the loader)
- Test: `packages/core/src/__tests__/config.test.ts`

**Interfaces:**
- Produces: `loadProjectProtectedWorktrees(projectDir: string): string[]`

- [ ] **Step 1: Write the failing tests**

Append to `packages/core/src/__tests__/config.test.ts` (the file already imports `mkdtempSync`, `writeFileSync`, `tmpdir`, `join`, `describe`, `expect`, `it`). Add `loadProjectProtectedWorktrees` to the existing import block from `../config.js`:

```ts
describe("loadProjectProtectedWorktrees", () => {
	it("reads a string list from vars.yaml", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-pw-"));
		writeFileSync(
			join(dir, "vars.yaml"),
			"protected_worktrees:\n  - legal-lake\n  - testing1\n",
		);
		expect(loadProjectProtectedWorktrees(dir)).toEqual([
			"legal-lake",
			"testing1",
		]);
	});

	it("returns [] for absent file or absent key", () => {
		const absent = mkdtempSync(join(tmpdir(), "queohoh-pw-"));
		expect(loadProjectProtectedWorktrees(absent)).toEqual([]);

		const noKey = mkdtempSync(join(tmpdir(), "queohoh-pw-"));
		writeFileSync(join(noKey, "vars.yaml"), "ticket: JUS-1\n");
		expect(loadProjectProtectedWorktrees(noKey)).toEqual([]);
	});

	it("tolerates a non-list value and skips non-string/empty entries", () => {
		const scalar = mkdtempSync(join(tmpdir(), "queohoh-pw-"));
		writeFileSync(join(scalar, "vars.yaml"), "protected_worktrees: legal-lake\n");
		expect(loadProjectProtectedWorktrees(scalar)).toEqual([]);

		const mixed = mkdtempSync(join(tmpdir(), "queohoh-pw-"));
		writeFileSync(
			join(mixed, "vars.yaml"),
			"protected_worktrees:\n  - legal-lake\n  - ''\n  - 12345\n",
		);
		expect(loadProjectProtectedWorktrees(mixed)).toEqual(["legal-lake"]);

		const notMap = mkdtempSync(join(tmpdir(), "queohoh-pw-"));
		writeFileSync(join(notMap, "vars.yaml"), "[not, a, map]\n");
		expect(loadProjectProtectedWorktrees(notMap)).toEqual([]);
	});

	it("is not surfaced as a template var by loadProjectVars", () => {
		const dir = mkdtempSync(join(tmpdir(), "queohoh-pw-"));
		writeFileSync(
			join(dir, "vars.yaml"),
			"ticket: JUS-1\nprotected_worktrees:\n  - legal-lake\n",
		);
		expect(loadProjectVars(dir)).toEqual({ ticket: "JUS-1" });
	});
});
```

`loadProjectVars` is already imported at the top of the test file (it is used elsewhere); confirm it is in the import list and add it if missing.

- [ ] **Step 2: Run tests to verify they fail**

Run: `pnpm --filter @queohoh/core exec vitest run -t "loadProjectProtectedWorktrees"`
Expected: FAIL — `loadProjectProtectedWorktrees is not a function` (and the `loadProjectVars` case fails: it currently returns `protected_worktrees` as a stringified value).

- [ ] **Step 3: Add the reserved-skip line in `loadProjectVars`**

In `packages/core/src/config.ts`, in the `for` loop of `loadProjectVars` (after the `default_model` skip at :131), add:

```ts
		if (key === "protected_worktrees") continue; // reserved: read by loadProjectProtectedWorktrees
```

- [ ] **Step 4: Add the loader**

In `packages/core/src/config.ts`, after `loadProjectDefaultModel` (ends at :191), add:

```ts
/** The project's optional `protected_worktrees` from vars.yaml — worktree names
 * that queohoh must never delete (on top of the always-protected main checkout).
 * Tolerant like loadProjectModels/loadProjectGithubId: absent file, absent key,
 * or a non-list value all yield [], and within a list any non-string or empty
 * entry is skipped. It never throws, so a malformed value only disables the
 * extra protections (the main checkout stays protected via path-equality) rather
 * than wedging config loading or snapshot generation. */
export function loadProjectProtectedWorktrees(projectDir: string): string[] {
	const path = join(projectDir, "vars.yaml");
	if (!existsSync(path)) return [];
	const raw = yaml.load(readFileSync(path, "utf-8")) ?? {};
	if (raw === null || typeof raw !== "object" || Array.isArray(raw)) return [];
	const value = (raw as Record<string, unknown>).protected_worktrees;
	if (!Array.isArray(value)) return [];
	return value.filter((v): v is string => typeof v === "string" && v.length > 0);
}
```

- [ ] **Step 5: Export from the barrel**

In `packages/core/src/index.ts`, add `loadProjectProtectedWorktrees` to the alphabetical export block from `./config.js` (between `loadProjectModels` at :10 and `loadProjectVars` at :10, so after `loadProjectModels,`):

```ts
	loadProjectModels,
	loadProjectProtectedWorktrees,
	loadProjectVars,
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `pnpm --filter @queohoh/core exec vitest run -t "loadProjectProtectedWorktrees"`
Expected: PASS (4 tests).

- [ ] **Step 7: Commit**

```bash
command git add packages/core/src/config.ts packages/core/src/index.ts packages/core/src/__tests__/config.test.ts
command git commit -m "feat(core): loadProjectProtectedWorktrees vars.yaml loader"
```

---

### Task 2: Core — `WorktreeInfo.protected` + `isProtectedWorktree` predicate

**Files:**
- Modify: `packages/core/src/resolver.ts:13-36` (add field), and add `isProtectedWorktree` after the `WorktreeInfo` interface
- Modify: `packages/core/src/index.ts:44-45` (export the predicate)
- Test: `packages/core/src/__tests__/resolver.test.ts`

**Interfaces:**
- Consumes: `WorktreeInfo` (from Task's own file)
- Produces:
  - `WorktreeInfo.protected?: boolean`
  - `isProtectedWorktree(repoPath: string | null, protectedNames: string[], wt: WorktreeInfo): boolean`

- [ ] **Step 1: Write the failing tests**

Append to `packages/core/src/__tests__/resolver.test.ts`. Add `isProtectedWorktree` and the `WorktreeInfo` type to the imports from `../resolver.js`:

```ts
describe("isProtectedWorktree", () => {
	const wt = (name: string, path: string): WorktreeInfo => ({
		name,
		path,
		branch: name,
	});

	it("protects the main checkout by path-equality even when name differs", () => {
		const repoPath = "/repos/platform";
		// worktree name is basename(path) = "platform" here, but the guard is by path
		expect(
			isProtectedWorktree(repoPath, [], wt("platform", "/repos/platform")),
		).toBe(true);
		// a differently-named checkout at the same path is still the main checkout
		expect(
			isProtectedWorktree(repoPath, [], wt("main", "/repos/platform")),
		).toBe(true);
	});

	it("protects a worktree whose name is in the configured list", () => {
		expect(
			isProtectedWorktree("/repos/platform", ["legal-lake"], wt("legal-lake", "/repos/platform.legal-lake")),
		).toBe(true);
	});

	it("does not protect an unlisted feature worktree", () => {
		expect(
			isProtectedWorktree("/repos/platform", ["legal-lake"], wt("JUS-1", "/repos/platform.JUS-1")),
		).toBe(false);
	});

	it("tolerates a null repoPath (no path match, list still applies)", () => {
		expect(isProtectedWorktree(null, [], wt("JUS-1", "/x"))).toBe(false);
		expect(isProtectedWorktree(null, ["JUS-1"], wt("JUS-1", "/x"))).toBe(true);
	});
});
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `pnpm --filter @queohoh/core exec vitest run -t "isProtectedWorktree"`
Expected: FAIL — `isProtectedWorktree is not a function`.

- [ ] **Step 3: Add the field and the predicate**

In `packages/core/src/resolver.ts`, add to the `WorktreeInfo` interface (after `prUrl?` at :35, before the closing `}` at :36):

```ts
	/** True when queohoh must never delete this worktree — the project's main
	 * checkout (path-equality) or a name in the project's `protected_worktrees`.
	 * Computed by the daemon and carried to the TUI. Absent/undefined = not
	 * protected (an old daemon that predates the field). */
	protected?: boolean;
```

Then, immediately after the `WorktreeInfo` interface closes (after :36), add:

```ts
/**
 * Whether `wt` is protected from deletion: it is the project's main checkout
 * (its path equals the project's registered checkout path) OR its name is in the
 * project's configured `protected_worktrees`. Path-equality — not name equality —
 * identifies the main checkout, because a project's name is a user label while a
 * worktree's name is `basename(path)`; the two can differ. `repoPath` is null for
 * an unknown repo, in which case only the name list applies.
 */
export function isProtectedWorktree(
	repoPath: string | null,
	protectedNames: string[],
	wt: WorktreeInfo,
): boolean {
	if (repoPath !== null && wt.path === repoPath) return true;
	return protectedNames.includes(wt.name);
}
```

- [ ] **Step 4: Export from the barrel**

In `packages/core/src/index.ts`, extend the `./resolver.js` value export (:45) to include the predicate:

```ts
export { isProtectedWorktree, REPO_SENTINEL, resolveTarget } from "./resolver.js";
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `pnpm --filter @queohoh/core exec vitest run -t "isProtectedWorktree"`
Expected: PASS (4 tests).

- [ ] **Step 6: Commit**

```bash
command git add packages/core/src/resolver.ts packages/core/src/index.ts packages/core/src/__tests__/resolver.test.ts
command git commit -m "feat(core): WorktreeInfo.protected field + isProtectedWorktree predicate"
```

---

### Task 3: Daemon — emit `protected` in `worktreesByRepo`

**Files:**
- Modify: `packages/daemon/src/engine.ts:16-35` (imports), `:118-127` (`worktreesByRepo`)
- Test: `packages/daemon/src/__tests__/engine.test.ts:389-413` (update existing exact-equal test) + new test

**Interfaces:**
- Consumes: `loadProjectProtectedWorktrees`, `isProtectedWorktree`, `projectWorkspaceDir` (Tasks 1-2), `this.repoPath`, `this.deps.config`
- Produces: every worktree object from `worktreesByRepo()` carries `protected: boolean`

- [ ] **Step 1: Update the existing exact-equal test + add a protected test**

In `packages/daemon/src/__tests__/engine.test.ts`, the test at :390 uses `toEqual` with an exact object. Add `protected: false` to the expected worktree object (after `prUrl: null,` at :409):

```ts
					prNumber: null,
					prUrl: null,
					protected: false,
```

Then add a new test inside the `describe("Engine.worktreesByRepo", ...)` block (after the existing `it` closes at :413):

```ts
	it("marks the main checkout and configured names as protected", async () => {
		const base = mkdtempSync(join(tmpdir(), "qo-eng-prot-"));
		const repoPath = join(base, "repo");
		mkdirSync(repoPath, { recursive: true });
		// vars.yaml under <workspace>/<project> protects the "legal-lake" worktree
		const wsProject = join(base, "ws", "platform");
		mkdirSync(wsProject, { recursive: true });
		writeFileSync(
			join(wsProject, "vars.yaml"),
			"protected_worktrees:\n  - legal-lake\n",
		);
		const { engine } = setup({
			config: { workspace: join(base, "ws"), projects: [{ name: "platform", path: repoPath }] },
			resolverIO: {
				listWorktrees: async () => [
					// main checkout: path === repoPath
					{ name: "platform", path: repoPath, branch: "main" },
					{ name: "legal-lake", path: join(base, "wt-ll"), branch: "legal-lake" },
					{ name: "JUS-1", path: join(base, "wt-jus1"), branch: "JUS-1" },
				],
			},
		});
		await engine.tick();
		const list = engine.worktreesByRepo().platform ?? [];
		const byName = Object.fromEntries(list.map((w) => [w.name, w.protected]));
		expect(byName).toEqual({ platform: true, "legal-lake": true, "JUS-1": false });
	});
```

Confirm `mkdirSync` and `writeFileSync` are imported at the top of `engine.test.ts` (the `setup` helper already uses `mkdirSync`; add `writeFileSync` to the `node:fs` import if absent).

- [ ] **Step 2: Run tests to verify they fail**

Run: `pnpm --filter @queohoh/daemon exec vitest run -t "worktreesByRepo"`
Expected: FAIL — the exact-equal test fails on the missing `protected` key, and the new test fails (`protected` is `undefined`, not the expected booleans).

- [ ] **Step 3: Add imports**

In `packages/daemon/src/engine.ts`, add to the value import block from `@queohoh/core` (:16-35), in alphabetical position:

```ts
	isProtectedWorktree,
```
(after `instantiateDefinition,` at :23) and
```ts
	loadProjectProtectedWorktrees,
```
(after `loadProjectModels,` at :25).

- [ ] **Step 4: Enrich in `worktreesByRepo`**

Replace the body of `worktreesByRepo()` (`engine.ts:118-127`) with:

```ts
	worktreesByRepo(): Record<string, WorktreeInfo[]> {
		const out: Record<string, WorktreeInfo[]> = {};
		for (const [repo, list] of this.worktreeCache) {
			const repoPath = this.repoPath(repo);
			const protectedNames = loadProjectProtectedWorktrees(
				projectWorkspaceDir(this.deps.config, repo),
			);
			out[repo] = list.map((wt) => {
				const e = this.gitEnrichCache.get(wt.path);
				const base: WorktreeInfo = e ? { ...wt, ...e } : { ...wt };
				base.protected = isProtectedWorktree(repoPath, protectedNames, wt);
				return base;
			});
		}
		return out;
	}
```

(`projectWorkspaceDir` is already imported at `engine.ts:28`.)

- [ ] **Step 5: Run tests to verify they pass**

Run: `pnpm --filter @queohoh/daemon exec vitest run -t "worktreesByRepo"`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
command git add packages/daemon/src/engine.ts packages/daemon/src/__tests__/engine.test.ts
command git commit -m "feat(daemon): emit protected flag per worktree in snapshot"
```

---

### Task 4: Daemon — hard-block guard in `removeWorktree`

**Files:**
- Modify: `packages/daemon/src/engine.ts:191-206` (`removeWorktree`)
- Test: `packages/daemon/src/__tests__/engine.test.ts`

**Interfaces:**
- Consumes: `loadProjectProtectedWorktrees`, `isProtectedWorktree`, `projectWorkspaceDir` (already imported in Task 3)

- [ ] **Step 1: Write the failing tests**

Add a new describe block to `packages/daemon/src/__tests__/engine.test.ts` (near the other `removeWorktree` tests; the `setup` helper is in scope):

```ts
describe("Engine.removeWorktree protection", () => {
	function protSetup() {
		const base = mkdtempSync(join(tmpdir(), "qo-eng-rm-prot-"));
		const repoPath = join(base, "repo");
		mkdirSync(repoPath, { recursive: true });
		const wsProject = join(base, "ws", "platform");
		mkdirSync(wsProject, { recursive: true });
		writeFileSync(
			join(wsProject, "vars.yaml"),
			"protected_worktrees:\n  - legal-lake\n",
		);
		let removed: string | null = null;
		const { engine } = setup({
			config: { workspace: join(base, "ws"), projects: [{ name: "platform", path: repoPath }] },
			resolverIO: {
				listWorktrees: async () => [
					{ name: "platform", path: repoPath, branch: "main" },
					{ name: "legal-lake", path: join(base, "wt-ll"), branch: "legal-lake" },
					{ name: "JUS-1", path: join(base, "wt-jus1"), branch: "JUS-1" },
				],
				removeWorktree: async (_r, wt) => {
					removed = wt.name;
				},
			},
		});
		return { engine, removed: () => removed };
	}

	it("refuses to remove the main checkout", async () => {
		const { engine, removed } = protSetup();
		await expect(engine.removeWorktree("platform", "platform")).rejects.toThrow(
			/protected/,
		);
		expect(removed()).toBeNull();
	});

	it("refuses to remove a configured protected worktree", async () => {
		const { engine, removed } = protSetup();
		await expect(engine.removeWorktree("platform", "legal-lake")).rejects.toThrow(
			/protected/,
		);
		expect(removed()).toBeNull();
	});

	it("still removes an unprotected worktree", async () => {
		const { engine, removed } = protSetup();
		await engine.removeWorktree("platform", "JUS-1");
		expect(removed()).toBe("JUS-1");
	});
});
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `pnpm --filter @queohoh/daemon exec vitest run -t "removeWorktree protection"`
Expected: FAIL — the two "refuses" tests fail (no guard yet, so `removeWorktree` succeeds and `removed()` is set).

- [ ] **Step 3: Add the guard**

In `packages/daemon/src/engine.ts` `removeWorktree`, after the `if (!wt) throw ...` line (:198) and before the busy-guard (`const lanes = ...` at :199), insert:

```ts
		const protectedNames = loadProjectProtectedWorktrees(
			projectWorkspaceDir(this.deps.config, repo),
		);
		if (isProtectedWorktree(repoPath, protectedNames, wt)) {
			throw new Error(`Worktree "${wt.name}" is protected and cannot be removed`);
		}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `pnpm --filter @queohoh/daemon exec vitest run -t "removeWorktree"`
Expected: PASS (protection tests + the pre-existing removeWorktree tests).

- [ ] **Step 5: Commit**

```bash
command git add packages/daemon/src/engine.ts packages/daemon/src/__tests__/engine.test.ts
command git commit -m "feat(daemon): refuse removal of protected worktrees in removeWorktree"
```

---

### Task 5: TUI — `protected` on the Rust `WorktreeInfo`

**Files:**
- Modify: `crates/qoo-tui/src/ipc/types.rs:135-166` (struct), `:335-419` (wire tests)

**Interfaces:**
- Produces: `WorktreeInfo.protected: bool` (serde `default`)

- [ ] **Step 1: Write the failing test**

In `crates/qoo-tui/src/ipc/types.rs`, extend the existing `modern_json()` fixture so one worktree carries `"protected": true`, and add an assertion in `deserializes_a_full_modern_snapshot` (around :376-419) plus a back-compat assertion. Concretely, add this focused test to the `types.rs` test module:

```rust
    #[test]
    fn worktree_protected_defaults_false_and_parses_true() {
        // Absent (old daemon) → false.
        let old: WorktreeInfo =
            serde_json::from_str(r#"{"name":"a","path":"/a","branch":"a"}"#).unwrap();
        assert!(!old.protected);
        // Present → parsed.
        let new: WorktreeInfo = serde_json::from_str(
            r#"{"name":"a","path":"/a","branch":"a","protected":true}"#,
        )
        .unwrap();
        assert!(new.protected);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p qoo-tui worktree_protected_defaults_false_and_parses_true`
Expected: FAIL to compile — `no field `protected` on type `WorktreeInfo``.

- [ ] **Step 3: Add the field**

In `crates/qoo-tui/src/ipc/types.rs`, add to `struct WorktreeInfo` (after `pub pr_url: Option<String>,` at :165, before the closing `}` at :166):

```rust
    /// True when queohoh must never delete this worktree (the project's main
    /// checkout or a name in the project's `protected_worktrees`). Absent on an
    /// old daemon → `false` via the container `default` (removable affordance;
    /// the daemon guard is the real block).
    pub protected: bool,
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p qoo-tui worktree_protected_defaults_false_and_parses_true`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
command git add crates/qoo-tui/src/ipc/types.rs
command git commit -m "feat(tui): protected field on WorktreeInfo (serde default false)"
```

---

### Task 6: TUI — `protected` on `WorktreeRow`

**Files:**
- Modify: `crates/qoo-tui/src/selectors.rs:48-87` (struct), `:533-554` (`worktree_rows` real-row map)
- Test: `crates/qoo-tui/src/selectors.rs` test module (near :2849)

**Interfaces:**
- Consumes: `WorktreeInfo.protected` (Task 5)
- Produces: `WorktreeRow.protected: bool` (session rows default `false`)

- [ ] **Step 1: Write the failing test**

Add to the `selectors.rs` test module (mirroring the existing `worktree_rows` tests around :2849). Build a snapshot with a protected worktree and assert the row carries it:

```rust
    #[test]
    fn worktree_rows_carry_protected_flag() {
        use crate::ipc::types::{Project, StateSnapshot, WorktreeInfo};
        use std::collections::HashMap;
        let mut wts = HashMap::new();
        wts.insert(
            "platform".to_string(),
            vec![
                WorktreeInfo {
                    name: "legal-lake".into(),
                    path: "/repos/platform.legal-lake".into(),
                    branch: "legal-lake".into(),
                    protected: true,
                    ..Default::default()
                },
                WorktreeInfo {
                    name: "JUS-1".into(),
                    path: "/repos/platform.JUS-1".into(),
                    branch: "JUS-1".into(),
                    ..Default::default()
                },
            ],
        );
        let s = StateSnapshot {
            projects: vec![Project { name: "platform".into(), github_id: None }],
            worktrees: wts,
            ..Default::default()
        };
        let rows = worktree_rows(&s, "platform");
        let by: std::collections::HashMap<_, _> =
            rows.iter().map(|r| (r.raw_name.clone(), r.protected)).collect();
        assert_eq!(by["legal-lake"], true);
        assert_eq!(by["JUS-1"], false);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p qoo-tui worktree_rows_carry_protected_flag`
Expected: FAIL to compile — `no field `protected` on type `WorktreeRow``.

- [ ] **Step 3: Add the field and copy it**

In `crates/qoo-tui/src/selectors.rs`, add to `struct WorktreeRow` (after `pub pr_url: Option<String>,` at :86, before the closing `}` at :87):

```rust
    /// True when the daemon flagged this worktree as protected from deletion.
    /// Drives the 🔒 marker and gates the remove action. Session rows default
    /// `false` (never removable anyway).
    pub protected: bool,
```

Then in `worktree_rows`, in the real-worktree `WorktreeRow { ... }` construction (`:533-554`), add after `pr_url: wt.pr_url.clone(),` (:553):

```rust
                protected: wt.protected,
```

The session-row branch (`:580-590`) uses `..Default::default()`, which covers `protected: false` — no change needed there.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p qoo-tui worktree_rows_carry_protected_flag`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
command git add crates/qoo-tui/src/selectors.rs
command git commit -m "feat(tui): carry protected flag onto WorktreeRow"
```

---

### Task 7: TUI — render 🔒 in the marker slot

**Files:**
- Modify: `crates/qoo-tui/src/view/theme.rs:49` area (add `GLYPH_PROTECTED`)
- Modify: `crates/qoo-tui/src/view/panes.rs:540-547` (marker block in `worktree_line`); confirm the `GLYPH_PROTECTED` import in `panes.rs` alongside `GLYPH_DIRTY`
- Test: `crates/qoo-tui/src/view/panes.rs` test module (near :1264)

**Interfaces:**
- Consumes: `WorktreeRow.protected` (Task 6)
- Produces: `pub const GLYPH_PROTECTED: char`

- [ ] **Step 1: Write the failing test**

Add to the `panes.rs` test module (`mod tests` at :1264). It renders a `worktree_line` for a protected row and asserts the padlock is present, plus a control row without it:

```rust
    #[test]
    fn worktree_line_shows_lock_for_protected_row() {
        use crate::selectors::{wt_col_layout, WorktreeRow};
        let p = Palette::default();
        let protected = WorktreeRow {
            name: "legal-lake".into(),
            raw_name: "legal-lake".into(),
            path: "/x".into(),
            branch: "legal-lake".into(),
            protected: true,
            ..Default::default()
        };
        let plain = WorktreeRow {
            name: "JUS-1".into(),
            raw_name: "JUS-1".into(),
            path: "/y".into(),
            branch: "JUS-1".into(),
            ..Default::default()
        };
        let rows = vec![protected.clone(), plain.clone()];
        let layout = wt_col_layout(&rows, 120);
        let text = |r: &WorktreeRow| {
            worktree_line(r, &layout, &p, 0)
                .spans
                .iter()
                .map(|s| s.content.clone())
                .collect::<String>()
        };
        assert!(text(&protected).contains(super::GLYPH_PROTECTED));
        assert!(!text(&plain).contains(super::GLYPH_PROTECTED));
    }
```

(`super::GLYPH_PROTECTED` resolves via the `use super::*;` at the top of the test module, which re-exports `panes.rs`'s imports. If clippy complains about the path, use `crate::view::theme::GLYPH_PROTECTED`.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p qoo-tui worktree_line_shows_lock_for_protected_row`
Expected: FAIL to compile — `GLYPH_PROTECTED` unresolved.

- [ ] **Step 3: Add the glyph constant**

In `crates/qoo-tui/src/view/theme.rs`, after `GLYPH_DIRTY` (:49), add:

```rust
/// Worktree is protected from deletion (main checkout or configured
/// `protected_worktrees`). Double-width emoji — it fills the whole 2-cell front
/// marker slot (glyph + separator), same as `GLYPH_SEARCH`.
pub const GLYPH_PROTECTED: char = '🔒';
```

- [ ] **Step 4: Render it in the marker slot**

In `crates/qoo-tui/src/view/panes.rs`, ensure `GLYPH_PROTECTED` is imported next to `GLYPH_DIRTY` (find the `use crate::view::theme::{... GLYPH_DIRTY ...}` line and add `GLYPH_PROTECTED`). Then replace the marker block (`:540-547`):

```rust
    if layout.dirty_w > 0 {
        if row.protected {
            // 🔒 is 2 display columns — it fills the whole [glyph][space] slot,
            // so no trailing space. Protected wins over the dirty ± marker.
            spans.push(Span::styled(GLYPH_PROTECTED.to_string(), meta));
        } else {
            if row.dirty == Some(true) {
                spans.push(Span::styled(GLYPH_DIRTY.to_string(), warn));
            } else {
                spans.push(Span::raw(" "));
            }
            spans.push(Span::raw(" "));
        }
    }
```

(`meta` is already bound in `worktree_line` at :532. If clippy flags the collapsible nested `if`, keep it as written for the two-cell symmetry — or flatten with `else if` — either passes tests; match the file's style.)

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p qoo-tui worktree_line_shows_lock_for_protected_row`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
command git add crates/qoo-tui/src/view/theme.rs crates/qoo-tui/src/view/panes.rs
command git commit -m "feat(tui): render lock glyph for protected worktrees"
```

---

### Task 8: TUI — gate the remove action

**Files:**
- Modify: `crates/qoo-tui/src/app/actions.rs:878-885` (single-select refusal in `remove_selected_worktree`)
- Modify: `crates/qoo-tui/src/app/menus.rs:59-67` (bulk eligibility filter in `open_bulk_menu`)
- Test: `crates/qoo-tui/src/app/bulk_flow_tests.rs`

**Interfaces:**
- Consumes: `WorktreeRow.protected` (Task 6)

- [ ] **Step 1: Write the failing tests**

Add to `crates/qoo-tui/src/app/bulk_flow_tests.rs`. It already has `app_with`, `key`, `shift_down`, and a `three_worktrees()` helper. Add a protected-aware fixture and two tests:

```rust
fn worktrees_with_protected() -> StateSnapshot {
    let mut wts = HashMap::new();
    wts.insert("platform".into(), vec![
        WorktreeInfo { name: "legal-lake".into(), path: "/wt/ll".into(), branch: "legal-lake".into(), protected: true, ..Default::default() },
        WorktreeInfo { name: "wt-b".into(), path: "/wt/b".into(), branch: "wt-b".into(), ..Default::default() },
    ]);
    StateSnapshot { projects: vec![Project { name: "platform".into(), github_id: None }], worktrees: wts, ..Default::default() }
}

#[test]
fn single_remove_refuses_a_protected_worktree() {
    let mut a = app_with(worktrees_with_protected());
    a.update(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))); // → tasks
    a.update(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))); // → worktrees
    // cursor on row 0 (legal-lake, protected) — press x
    let u = a.update(key('x'));
    assert!(matches!(a.mode, Mode::List), "no confirm dialog opens");
    assert_eq!(a.status_line.as_deref(), Some("worktree is protected"));
    assert!(!u.cmds.iter().any(|c| matches!(c, Cmd::Rpc { .. } | Cmd::RpcSeq { .. })));
}

#[test]
fn bulk_remove_drops_protected_rows() {
    let mut a = app_with(worktrees_with_protected());
    a.update(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))); // → tasks
    a.update(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))); // → worktrees
    a.update(shift_down()); // 2-row range: legal-lake(protected) + wt-b
    a.update(key('x')); // opens bulk confirm with only eligible rows
    match &a.mode {
        Mode::Confirm { action: ConfirmAction::BulkRemoveWorktrees { names, .. }, .. } => {
            assert_eq!(names, &vec!["wt-b".to_string()]); // protected dropped
        }
        other => panic!("{other:?}"),
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p qoo-tui -- single_remove_refuses_a_protected_worktree bulk_remove_drops_protected_rows`
Expected: FAIL — single-select opens the confirm dialog (no refusal yet); bulk includes `legal-lake`.

- [ ] **Step 3: Add the single-select refusal**

In `crates/qoo-tui/src/app/actions.rs` `remove_selected_worktree`, after the `is_session` refusal (:878-881) and before the `Busy` check (:882), insert:

```rust
        if row.protected {
            self.status_line = Some("worktree is protected".into());
            return Update { dirty: true, cmds: vec![] };
        }
```

- [ ] **Step 4: Add the bulk filter**

In `crates/qoo-tui/src/app/menus.rs` `open_bulk_menu`, extend the eligibility `.filter(...)` (:65) to also drop protected rows:

```rust
                        .filter(|r| {
                            !r.is_session
                                && !matches!(r.state, crate::selectors::WtState::Busy)
                                && !r.protected
                        })
```

(The existing empty-after-filter branch at :68-71 already produces `"no eligible rows"`, giving the all-protected-selection no-op for free.)

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p qoo-tui -- single_remove_refuses_a_protected_worktree bulk_remove_drops_protected_rows`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
command git add crates/qoo-tui/src/app/actions.rs crates/qoo-tui/src/app/menus.rs crates/qoo-tui/src/app/bulk_flow_tests.rs
command git commit -m "feat(tui): gate remove action on protected worktrees"
```

---

### Task 9: Full gate + config data

**Files:**
- Modify: `~/workspace/queohoh/platform/vars.yaml` (user config repo — add the two protected names)

- [ ] **Step 1: Run the full gate**

Run: `mise run check`
Expected: PASS — TS test + typecheck + `lint:ci`, Rust `test:rs` + `check` + `lint:rs` (clippy `-D warnings`). Fix any clippy findings (e.g. collapsible-if in Task 7) inline and re-run.

- [ ] **Step 2: Add the platform protected worktrees to the config repo**

Append to `~/workspace/queohoh/platform/vars.yaml`:

```yaml

# Worktrees queohoh must never delete (the main checkout is always protected).
protected_worktrees:
  - legal-lake
  - testing1
```

This is the user's config repo, not this source repo — no code test. Verify by launching the TUI (`mise run tui`) and confirming `legal-lake` and `testing1` (and the `platform` main checkout) show 🔒 and refuse the `x` remove.

- [ ] **Step 3: Commit the config repo (separate repo)**

```bash
command git -C ~/workspace/queohoh add platform/vars.yaml
command git -C ~/workspace/queohoh commit -m "config(platform): protect legal-lake and testing1 worktrees"
```

---

## Self-Review

**Spec coverage:**
- Config surface (per-project `vars.yaml` list) → Task 1 + Task 9.
- Protection predicate (path-equality main checkout + name list) → Task 2.
- Engine hard block → Task 4.
- Snapshot enrichment → Task 3.
- TUI deserialize → Task 5; row state → Task 6; lock glyph (reused marker slot) → Task 7; single + bulk gating → Task 8.
- Tolerant loader / error handling → Task 1 (loader never throws; no call-site guards needed).
- Tests enumerated per section → Tasks 1-8; full gate → Task 9.

**Placeholder scan:** none — every code step shows complete code and exact commands.

**Type consistency:** `loadProjectProtectedWorktrees(projectDir): string[]` and `isProtectedWorktree(repoPath, protectedNames, wt): boolean` are used identically in Tasks 3-4; Rust `protected: bool` field name matches across `WorktreeInfo` (Task 5), `WorktreeRow` (Task 6), render (Task 7), and gating (Task 8); serde `camelCase` maps TS `protected` ↔ Rust `protected` (same word, unaffected by case).
