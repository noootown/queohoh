# Autotest Platform Task Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the queohoh `autotest` platform task with managed stack lifecycle, a port-keyed mutex shared by all e2e actors, a daemon `lane:` override for definition-level serialization, the pr-review `static_review`/`e2e_review` extension, the offline `self-review-e2e` task, and the `/autotest` skill refactor.

**Architecture:** A small daemon change adds an optional `lane:` field to task definitions so all `autotest` instances serialize through one scheduler lane. The config repo gains a shared testing core (`autotest-core.md`), a port-keyed mkdir-lock helper (`stack-lock.sh`), and three task definitions (new `autotest`, new `self-review-e2e`, extended `pr-review`). The personal `/autotest` skill is refactored to consume the shared core and acquire the port lock.

**Tech Stack:** TypeScript (zod, vitest, pnpm workspaces) for the daemon; bash + YAML + markdown prompts for the config repo; markdown for the skill.

**Spec:** `docs/superpowers/specs/2026-07-16-autotest-platform-task-design.md` (this repo). Read it before starting.

## Global Constraints

- Three working directories. Phase A: `/Users/noootown/Downloads/agent247/queohoh` (daemon source, its own git repo). Phases B–D: `/Users/noootown/workspace/queohoh/platform` (config) and `/Users/noootown/workspace/claude-code/skills/autotest` (skill) — **both live in the git repo rooted at `/Users/noootown/workspace`**; commit from there with explicit paths.
- **Ordering is load-bearing:** Phase A must be fully built AND the daemon restarted before Task 7 lands `lane:` in a live config.yaml — `DefinitionConfigSchema` is `.strict()`, so an old daemon skips the whole definition as unparseable.
- Never add `Co-Authored-By` trailers to commits (user rule).
- Markdown files: one logical line per paragraph/bullet — never hard-wrap at ~80 cols (user rule).
- Lock root path (all actors): `$HOME/workspace/queohoh/platform/state`, overridable via `STACK_LOCK_ROOT` (tests use the override). Lock dir name: `stack-port-<PORT>.lock`.
- Stack identity port = `TRAEFIK_HTTPS_PORT` from the relevant `.env.worktree`; `3443` when the file or var is absent. testing1's is `4343` (offset +900).
- Verdict vocabulary everywhere: `✅ works` / `⚠️ partial` / `❌ failed` / `🚧 blocked`.
- PR comments (autotest with `post_pr=true`) never include 🚧 blocked rows; all-blocked → no comment.
- Daemon commands: `pnpm -r test`, `pnpm -r typecheck`, `pnpm lint:ci`, full gate `mise run check`, restart via `bash scripts/daemon-ensure.sh`.

---

## Phase A — daemon `lane:` override (cwd: `/Users/noootown/Downloads/agent247/queohoh`)

### Task 1: `lane` on TaskDefinition

**Files:**
- Modify: `packages/core/src/definition.ts` (schema ~line 40-67, interface ~line 69-85, loadDefinition ~line 148)
- Test: `packages/core/src/__tests__/definition.test.ts`

**Interfaces:**
- Produces: `TaskDefinition.lane: string | null` — consumed by Tasks 3 (instantiate/api stamping).

- [ ] **Step 1: Write the failing test**

Add to `packages/core/src/__tests__/definition.test.ts` inside `describe("loadDefinition", ...)`:

```typescript
it("loads a lane override, null when absent", () => {
	const projectDir = makeRepo({
		autotest: {
			config: "lane: testing1-stack",
			prompt: "Test.\n",
		},
		plain: {
			config: "description: no lane here",
			prompt: "P.\n",
		},
	});
	expect(loadDefinition(projectDir, "platform", "autotest").lane).toBe("testing1-stack");
	expect(loadDefinition(projectDir, "platform", "plain").lane).toBeNull();
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm --filter @queohoh/core test -- definition`
Expected: FAIL — `lane` is not a property (TS error) or `undefined !== "testing1-stack"`. Also expect a compile error on the schema (unknown key `lane` under `.strict()` makes `loadDefinition` throw `Unrecognized key`) — that throw IS the current behavior the feature removes.

- [ ] **Step 3: Implement**

In `packages/core/src/definition.ts`:

Add to `DefinitionConfigSchema` (after the `worktree` field, before `pre_run`):

```typescript
			// Optional scheduler-lane override. When set, every instance of this
			// definition shares one lane (`repo:<lane>`) instead of the default
			// per-worktree lane — serializing runs across different worktrees.
			// Motivating case: the autotest task always spawns a stack on
			// testing1's ports, so two instances must never run concurrently even
			// though each lives in its own PR worktree.
			lane: z.string().min(1).optional(),
```

Add to `interface TaskDefinition` (after `worktree: string;`):

```typescript
	/** Scheduler-lane override; null = default per-worktree lane. See the
	 * schema comment — serializes all instances of this definition. */
	lane: string | null;
```

Add to the return object in `loadDefinition` (after `worktree: config.worktree,`):

```typescript
		lane: config.lane ?? null,
```

- [ ] **Step 4: Fix the full-object equality test and other TaskDefinition literals**

The first test in `definition.test.ts` (`loads a full definition with defaults applied`) uses `expect(def).toEqual({...})` — add `lane: null,` to that expected object (after `worktree: "pr:{{number}}",`).

Run: `pnpm --filter @queohoh/core typecheck`
Add `lane: null,` to every `TaskDefinition` literal the compiler flags. Expected flags: the `def()` helper in `packages/core/src/__tests__/instantiate.test.ts` (~line 10) and the equivalent helper in `packages/core/src/__tests__/worker.test.ts`. Check the daemon package too: `pnpm --filter @queohoh/daemon typecheck` (its tests may build definition literals, e.g. `__tests__/pr-review-shape.test.ts`).

- [ ] **Step 5: Run tests to verify they pass**

Run: `pnpm -r test && pnpm -r typecheck`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add packages/core/src/definition.ts packages/core/src/__tests__/
git commit -m "feat(core): optional lane override on task definitions"
```

(If daemon-package test files changed in Step 4, add those paths too.)

### Task 2: `lane` on TaskInstance + laneKey override

**Files:**
- Modify: `packages/core/src/task.ts` (TaskMetaSchema ~line 36-95, TaskInstance ~line 97-145, parseTaskFile ~line 147, serializeTaskFile ~line 179, laneKey ~line 209)
- Test: `packages/core/src/__tests__/task.test.ts`, `packages/core/src/__tests__/scheduler.test.ts`

**Interfaces:**
- Consumes: nothing new.
- Produces: `TaskInstance.lane?: string | null` and the new `laneKey` semantics: `laneKey(t)` returns `null` while `t.target.worktree === null` (resolve path unchanged), else `` `${repo}:${t.lane}` `` when `lane` is set, else `` `${repo}:${worktree}` ``.

- [ ] **Step 1: Write the failing tests**

Add to `packages/core/src/__tests__/task.test.ts`:

```typescript
describe("lane override", () => {
	it("round-trips lane through serialize/parse, defaulting null", () => {
		const t = parseTaskFile(
			serializeTaskFile({
				...parseTaskFile(serializeTaskFile(baseTask())),
				lane: "testing1-stack",
			}),
		);
		expect(t.lane).toBe("testing1-stack");
		expect(parseTaskFile(serializeTaskFile(baseTask())).lane).toBeNull();
	});

	it("laneKey uses the override only after the worktree resolves", () => {
		const unresolved = { ...baseTask(), lane: "testing1-stack" };
		unresolved.target = { ...unresolved.target, worktree: null };
		expect(laneKey(unresolved)).toBeNull();

		const resolved = { ...baseTask(), lane: "testing1-stack" };
		resolved.target = { ...resolved.target, worktree: "pr-101" };
		expect(laneKey(resolved)).toBe("platform:testing1-stack");

		const plain = baseTask();
		plain.target = { ...plain.target, worktree: "pr-101" };
		expect(laneKey(plain)).toBe("platform:pr-101");
	});
});
```

`baseTask()`: reuse the file's existing task-literal helper if one exists; otherwise add one mirroring the `task()` helper at the top of `scheduler.test.ts` (full `TaskInstance` literal, `repo: "platform"`).

Add to `packages/core/src/__tests__/scheduler.test.ts` (the `task()` helper needs a `lane?: string | null` override plumbed through — add `lane: overrides.lane ?? null,` to the returned literal and the override type):

```typescript
	it("serializes lane-override tasks across different worktrees", () => {
		const a = task({ worktree: "pr-101", lane: "testing1-stack" });
		const b = task({ worktree: "pr-202", lane: "testing1-stack" });
		const d = schedule([a, b], idle, { perProjectMax: 5 });
		expect(d.start).toEqual([a]); // b waits: same lane
	});

	it("lane-override task does not block unrelated worktree lanes", () => {
		const a = task({ worktree: "pr-101", lane: "testing1-stack" });
		const c = task({ worktree: "pr-101" }); // same worktree, default lane
		const d = schedule([a, c], idle, { perProjectMax: 5 });
		expect(d.start).toEqual([a, c]); // different lane keys → both start
	});
```

(The second test documents that an override task and a default-lane task in the same worktree do NOT collide at schedule time — the worktree itself still serializes at the lane level only for same-key tasks. This is acceptable: autotest is the only definition using the override and it owns its worktree during a run.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `pnpm --filter @queohoh/core test -- task scheduler`
Expected: FAIL — `lane` unknown on TaskInstance.

- [ ] **Step 3: Implement in `packages/core/src/task.ts`**

`TaskMetaSchema` — add after `attempted_providers`:

```typescript
		// Scheduler-lane override stamped from the definition's `lane:` at create
		// time (additive; absent on legacy files → null). See laneKey below.
		lane: z.string().nullable().default(null),
```

`TaskInstance` — add after `attemptedProviders: string[];`:

```typescript
	/** Scheduler-lane override stamped from the definition; null = default
	 * per-worktree lane. Optional so pre-lane callers and test literals need
	 * not set it. */
	lane?: string | null;
```

`parseTaskFile` — add `lane: m.lane,` to the returned object. `serializeTaskFile` — add `lane: task.lane ?? null,` to `meta`.

`laneKey` — replace the body:

```typescript
export function laneKey(task: TaskInstance): string | null {
	// Unresolved worktree → null lane, ALWAYS: the scheduler routes null-lane
	// tasks to worktree resolution, and a lane override must not skip that.
	if (task.target.worktree === null) return null;
	// Definition-level override: every instance of the definition shares one
	// lane, serializing runs across different worktrees (e.g. autotest, whose
	// stack always binds testing1's ports).
	if (task.lane) return `${task.target.repo}:${task.lane}`;
	return `${task.target.repo}:${task.target.worktree}`;
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `pnpm --filter @queohoh/core test -- task scheduler`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add packages/core/src/task.ts packages/core/src/__tests__/task.test.ts packages/core/src/__tests__/scheduler.test.ts
git commit -m "feat(core): persist lane override on tasks and honor it in laneKey"
```

### Task 3: stamp `lane` at every create path

**Files:**
- Modify: `packages/core/src/store.ts` (NewTaskInput ~line 18, create ~line 88, ChainStepInput ~line 43, createChain ~line 128)
- Modify: `packages/core/src/instantiate.ts` (store.create call ~line 107-121)
- Modify: `packages/daemon/src/api.ts` (chain-step builder ~line 373-408, definition-step branch)
- Test: `packages/core/src/__tests__/store.test.ts`, `packages/core/src/__tests__/instantiate.test.ts`

**Interfaces:**
- Consumes: `TaskDefinition.lane` (Task 1), `TaskInstance.lane` (Task 2).
- Produces: `NewTaskInput.lane?: string`, `ChainStepInput.lane?: string` — both flow onto the created `TaskInstance.lane`.

- [ ] **Step 1: Write the failing tests**

`packages/core/src/__tests__/store.test.ts`:

```typescript
	it("persists a lane override through create and reload", () => {
		const store = freshStore();
		const t = store.create({
			prompt: "x",
			repo: "platform",
			ref: "temp",
			source: "mcp",
			lane: "testing1-stack",
		});
		expect(t.lane).toBe("testing1-stack");
		expect(store.get(t.id)?.lane).toBe("testing1-stack");
		const plain = store.create({ prompt: "y", repo: "platform", ref: "temp", source: "mcp" });
		expect(plain.lane).toBeNull();
	});
```

`packages/core/src/__tests__/instantiate.test.ts` (args-mode, mirrors existing tests; `def()` already gained `lane: null` in Task 1 Step 4):

```typescript
	it("stamps the definition lane onto created instances", async () => {
		const store = freshStore();
		const created = await instantiateDefinition(
			def({ lane: "testing1-stack", discovery: null, args: [{ name: "number" }] }),
			{ mode: "args", values: ["7"] },
			{ ...deps(store, ""), source: "mcp" as const },
		);
		expect(created[0]?.lane).toBe("testing1-stack");
	});
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `pnpm --filter @queohoh/core test -- store instantiate`
Expected: FAIL — `lane` not accepted by `NewTaskInput` / instances have `lane: null`.

- [ ] **Step 3: Implement**

`store.ts` — `NewTaskInput` gains (after `verify?: string;`):

```typescript
	/** Scheduler-lane override from the definition's `lane:`; see task.ts. */
	lane?: string;
```

`create()` — in the `TaskInstance` literal add `lane: input.lane ?? null,` (after `attemptedProviders: [],`).

`ChainStepInput` gains the same `lane?: string;` field; `createChain()`'s member literal adds `lane: step.lane ?? null,`.

`instantiate.ts` — in the `deps.store.create({...})` call add `lane: def.lane ?? undefined,`.

`packages/daemon/src/api.ts` — in the chain-step builder's definition branch (the object returned after `const item = buildItemFromArgs(def, values);`) add `lane: def.lane ?? undefined,`. The prompt-step branch (`{ prompt: s.prompt, model, timeoutMs, verify }`) stays lane-less — ad-hoc prompts have no definition to inherit from.

- [ ] **Step 4: Run the full gate**

Run: `pnpm -r test && pnpm -r typecheck && pnpm lint:ci`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add packages/core/src/store.ts packages/core/src/instantiate.ts packages/daemon/src/api.ts packages/core/src/__tests__/store.test.ts packages/core/src/__tests__/instantiate.test.ts
git commit -m "feat: stamp definition lane onto tasks at every create path"
```

### Task 4: build + restart the daemon

- [ ] **Step 1: Full gate**

Run: `mise run check`
Expected: build, test, typecheck, lint all green (Rust TUI untouched; its tests must still pass unchanged).

- [ ] **Step 2: Restart the daemon on the new build**

Run: `bash scripts/daemon-ensure.sh`
Expected: daemon builds and (re)starts. Verify: `mise run status` (or the script's own output) shows the daemon up.

- [ ] **Step 3: Verify definitions still parse**

Run a definitions listing through the MCP (`mcp__queohoh__list_task_definitions`) or check the daemon log for `skipping unparseable definition` — there must be none.

---

## Phase B — config repo (cwd: `/Users/noootown/workspace/queohoh/platform`; commit from git root `/Users/noootown/workspace`)

### Task 5: `shared/stack-lock.sh`

**Files:**
- Create: `/Users/noootown/workspace/queohoh/platform/shared/stack-lock.sh`

**Interfaces:**
- Produces the CLI contract every later task uses:
  - `bash stack-lock.sh port` → prints this cwd's stack port (`TRAEFIK_HTTPS_PORT` from `./.env.worktree`, else `3443`)
  - `bash stack-lock.sh acquire <port> <owner-label>` → exit 0 acquired / exit 1 held (owner meta on stderr)
  - `bash stack-lock.sh release <port>`
  - `bash stack-lock.sh status <port>` → prints `meta.json` (exit 0) or `free` (exit 1)
- Env: `STACK_LOCK_ROOT` overrides the lock root (default `$HOME/workspace/queohoh/platform/state`); `STACK_LOCK_PID` overrides the recorded pid (default `$PPID` — the claude process driving the Bash call, a meaningful liveness proxy).

- [ ] **Step 1: Write the script**

```bash
#!/usr/bin/env bash
# stack-lock.sh — port-keyed mutex for e2e testing runs (autotest task,
# /autotest skill, self-review-e2e). One mkdir-lock per portal (Traefik
# HTTPS) port: two runs against the same stack exclude each other regardless
# of actor; runs against different-port stacks proceed concurrently.
# mkdir is the lock primitive because macOS ships no flock binary.
#
# Usage:
#   stack-lock.sh port                    print this cwd's stack port
#   stack-lock.sh acquire <port> <owner>  exit 0 acquired / 1 held (owner on stderr)
#   stack-lock.sh release <port>
#   stack-lock.sh status <port>           print meta.json (exit 0) or "free" (exit 1)
set -euo pipefail

LOCK_ROOT="${STACK_LOCK_ROOT:-$HOME/workspace/queohoh/platform/state}"

lock_dir() { echo "$LOCK_ROOT/stack-port-$1.lock"; }

cmd="${1:-}"
case "$cmd" in
  port)
    if [ -f .env.worktree ]; then
      p="$(sed -n 's/^TRAEFIK_HTTPS_PORT=\([0-9][0-9]*\)$/\1/p' .env.worktree | head -1)"
      if [ -n "$p" ]; then echo "$p"; exit 0; fi
    fi
    echo 3443
    ;;
  acquire)
    port="$2"; owner="$3"
    dir="$(lock_dir "$port")"
    mkdir -p "$LOCK_ROOT"
    if mkdir "$dir" 2>/dev/null; then
      printf '{"owner": "%s", "pid": %s, "worktree": "%s", "port": %s, "started": "%s"}\n' \
        "$owner" "${STACK_LOCK_PID:-$PPID}" "$(pwd)" "$port" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
        > "$dir/meta.json"
      exit 0
    fi
    meta="$dir/meta.json"
    if [ -f "$meta" ]; then
      opid="$(sed -n 's/.*"pid": \([0-9][0-9]*\).*/\1/p' "$meta")"
      if [ -n "$opid" ] && ! kill -0 "$opid" 2>/dev/null; then
        echo "stack-lock: port $port held by DEAD pid $opid — stale. Recover: rm -rf $dir" >&2
      fi
      echo "stack-lock: port $port held: $(cat "$meta")" >&2
    else
      echo "stack-lock: port $port held (no meta). Recover: rm -rf $dir" >&2
    fi
    exit 1
    ;;
  release)
    rm -rf "$(lock_dir "$2")"
    ;;
  status)
    dir="$(lock_dir "$2")"
    if [ -d "$dir" ]; then
      cat "$dir/meta.json" 2>/dev/null || echo '{}'
      exit 0
    fi
    echo "free"
    exit 1
    ;;
  *)
    echo "usage: stack-lock.sh port | acquire <port> <owner> | release <port> | status <port>" >&2
    exit 2
    ;;
esac
```

`chmod +x` the file.

- [ ] **Step 2: Test it standalone**

```bash
export STACK_LOCK_ROOT=$(mktemp -d)
S=/Users/noootown/workspace/queohoh/platform/shared/stack-lock.sh
bash $S status 4343                      # → "free", exit 1
bash $S acquire 4343 test-owner && echo ACQUIRED   # → ACQUIRED
bash $S acquire 4343 other; echo "exit=$?"          # → held message on stderr, exit=1
bash $S status 4343                      # → meta.json with owner "test-owner", exit 0
bash $S acquire 5343 other && echo PORT-INDEPENDENT # → different port acquires fine
bash $S release 4343 && bash $S status 4343         # → "free" again
STACK_LOCK_PID=99999999 bash $S acquire 4343 dead && bash -c "bash $S acquire 4343 x; true"  # → stderr includes "DEAD pid 99999999 — stale"
rm -rf "$STACK_LOCK_ROOT"; unset STACK_LOCK_ROOT
```

Also verify `port`: run `cd /Users/noootown/Downloads/projects/platform.testing1 && bash $S port` → `4343`; from a dir with no `.env.worktree` → `3443`.

- [ ] **Step 3: Commit (from `/Users/noootown/workspace`)**

```bash
git -C /Users/noootown/workspace add queohoh/platform/shared/stack-lock.sh
git -C /Users/noootown/workspace commit -m "feat(platform): port-keyed stack-lock helper for e2e runs"
```

### Task 6: `shared/autotest-core.md`

**Files:**
- Create: `/Users/noootown/workspace/queohoh/platform/shared/autotest-core.md`
- Source: `/Users/noootown/workspace/claude-code/skills/autotest/SKILL.md` (the subagent prompt template — everything inside the ```` ```` fenced block under "## Subagent prompt template")

**Interfaces:**
- Produces: a self-contained per-scenario testing instruction document with two substitution placeholders — `<SCENARIO>` and `<CWD>` — and the exact `Testing Result:` / `Output:` report shape. Consumers (Tasks 7, 8, 10) build a subagent prompt by cat'ing this file and substituting the placeholders.

- [ ] **Step 1: Extract the core**

Copy the ENTIRE subagent prompt template body from SKILL.md (starting at "You are an e2e tester. Your one job: …" through the end of the "### Verdict guide" section) into `autotest-core.md`, then make exactly these edits:

1. Prepend this header (replaces nothing — goes above the copied text):

```markdown
# autotest-core — shared e2e testing instructions

This is the single source of truth for how an e2e scenario is executed against a local JusticeBid stack. Consumers: the `/autotest` skill, the `autotest` platform task, and the `self-review-e2e` platform task. Each consumer builds a subagent prompt from this file, substituting `<SCENARIO>` (the scenario text) and `<CWD>` (the absolute worktree path). The stack must already be reachable — this document never starts or stops services; stack lifecycle belongs to the caller.
```

2. In the copied "## Setup" section, the line "Services are already running. Do NOT attempt to start them." stays — it is true for every consumer (the autotest task starts the stack BEFORE dispatching scenario subagents).

3. Append this note at the very end:

```markdown
## Multi-scenario callers

A caller running several scenarios dispatches one subagent per scenario, sequentially, each with its own `<SCENARIO>` substitution and its own run slug. The caller aggregates the per-scenario `Output:` blocks into a verdict table: one row per scenario — `| <scenario one-liner> | <verdict emoji + word> |`.
```

Make no other content edits — credential tables, login-bypass instructions, `mise run info` URL discovery, agent-browser flags, auth flow, screenshot policy, reseed rail, and the report templates are copied verbatim.

- [ ] **Step 2: Verify the extraction is complete**

```bash
grep -c "login_hint" /Users/noootown/workspace/queohoh/platform/shared/autotest-core.md   # ≥ 2
grep -c "Verdict guide" /Users/noootown/workspace/queohoh/platform/shared/autotest-core.md # 1
grep -c "mise run info" /Users/noootown/workspace/queohoh/platform/shared/autotest-core.md # ≥ 3
```

- [ ] **Step 3: Commit**

```bash
git -C /Users/noootown/workspace add queohoh/platform/shared/autotest-core.md
git -C /Users/noootown/workspace commit -m "feat(platform): extract shared autotest-core testing instructions"
```

### Task 7: `tasks/autotest/` — config, prompt, teardown

**Files:**
- Create: `/Users/noootown/workspace/queohoh/platform/tasks/autotest/config.yaml`
- Create: `/Users/noootown/workspace/queohoh/platform/tasks/autotest/prompt.md`
- Create: `/Users/noootown/workspace/queohoh/platform/tasks/autotest/teardown.sh`

**Interfaces:**
- Consumes: `stack-lock.sh` CLI (Task 5), `autotest-core.md` (Task 6), daemon `lane:` (Phase A).
- Produces: the `autotest` definition invoked by pr-review (Task 9) as `run_task_definition {repo: "platform", name: "autotest", args: [<target>, <scenarios>, <post_pr>], cwd: <pr worktree>}`.

- [ ] **Step 1: Write `config.yaml`**

```yaml
# autotest — managed-stack e2e testing. Spawns `mise run dev` in the target
# worktree using testing1's .env.worktree (the PR's code on testing1's
# ports/credentials), runs every scenario in one stack lifecycle, tears the
# stack down, and reports per-scenario verdicts. Only tests; posting a PR
# comment is opt-in via post_pr (pr-review passes "true", manual runs on
# your own branch default to "false" so the task is side-effect-free).
description: E2E-test scenarios against a self-spawned stack (testing1 env) in the target worktree
args:
  - name: target
    type: worktree
    description: worktree/PR whose code to test; inferred when launched from a worktree, or pick/type one
  - name: scenarios
    type: text
    description: 1..N named scenarios to run, typically /pr-tldr smoke-test output
  - name: post_pr
    default: "false"
    options: ["false", "true"]
    description: post the e2e report as a PR comment (failed/partial/works rows only; blocked stays local)
# All instances serialize through one lane — the stack always binds
# testing1's ports, so two runs can never overlap even across worktrees.
lane: testing1-stack
worktree: "worktree:{{target}}"
dedup: none # re-runnable — retest after new commits
# Belt-and-suspenders teardown: runs with cwd = the worktree even when the
# worker failed or timed out; releases the port lock, restores the env swap,
# and kills the spawned stack. Idempotent — a clean run leaves it a no-op.
post_run: bash {{queohoh_workspace}}/platform/tasks/autotest/teardown.sh
model: opus
timeout: 60m
priority: normal
```

- [ ] **Step 2: Write `teardown.sh`**

```bash
#!/usr/bin/env bash
# teardown.sh — post_run backstop for the autotest task. Idempotent: a clean
# run (graceful teardown already done) makes every step a no-op. Runs with
# cwd = the task's worktree (queohoh hook contract). Never uses `set -e` —
# each cleanup step must be attempted even if an earlier one fails.
set -uo pipefail

WT="$(pwd)"
LOCK_ROOT="${STACK_LOCK_ROOT:-$HOME/workspace/queohoh/platform/state}"

# 1. Find lock(s) whose meta.json says this worktree owns them. A lock owned
#    by a different worktree (or by "interactive") is left strictly alone.
owned=0
for dir in "$LOCK_ROOT"/stack-port-*.lock; do
  [ -d "$dir" ] || continue
  meta="$dir/meta.json"
  [ -f "$meta" ] || continue
  grep -qF "\"worktree\": \"$WT\"" "$meta" || continue
  owned=1
  rm -rf "$dir"
done

# 2. Env-swap markers exist only when the prompt's env-swap phase ran and the
#    graceful teardown didn't restore. Their presence means we spawned (or
#    were about to spawn) a stack in this worktree.
swapped=0
if [ -f "$WT/.env.worktree.autotest-bak" ]; then
  swapped=1
elif [ -f "$WT/.env.worktree.autotest-absent" ]; then
  swapped=1
fi

# 3. Kill the spawned stack — but only when this run demonstrably owned it
#    (lock or env marker present). dev:kill is the platform's own
#    worktree-scoped kill (discovers by OVERMIND_TITLE); safe here because
#    the only stack in THIS worktree during an autotest run is ours.
if [ "$owned" = 1 ] || [ "$swapped" = 1 ]; then
  (cd "$WT" && mise run dev:kill) || true
fi

# 4. Restore the env swap AFTER the kill (dev:kill may read the env in place).
if [ -f "$WT/.env.worktree.autotest-bak" ]; then
  mv -f "$WT/.env.worktree.autotest-bak" "$WT/.env.worktree"
elif [ -f "$WT/.env.worktree.autotest-absent" ]; then
  rm -f "$WT/.env.worktree" "$WT/.env.worktree.autotest-absent"
fi

exit 0
```

`chmod +x` the file.

- [ ] **Step 3: Test `teardown.sh` standalone**

```bash
export STACK_LOCK_ROOT=$(mktemp -d)
TD=/Users/noootown/workspace/queohoh/platform/tasks/autotest/teardown.sh
WORK=$(mktemp -d) && cd "$WORK"
bash "$TD" && echo "no-op OK"                                # no lock, no markers → exit 0, nothing happens (mise not invoked)
mkdir -p "$STACK_LOCK_ROOT/stack-port-4343.lock"
printf '{"owner": "task:autotest", "pid": 1, "worktree": "%s", "port": 4343, "started": "x"}\n' "$WORK" > "$STACK_LOCK_ROOT/stack-port-4343.lock/meta.json"
touch .env.worktree.autotest-absent && touch .env.worktree
bash "$TD"; ls "$STACK_LOCK_ROOT"                            # lock dir gone
test ! -f .env.worktree && echo "env removed OK"             # absent-marker path removed the copied env
mkdir -p "$STACK_LOCK_ROOT/stack-port-5343.lock"
printf '{"owner": "interactive", "pid": 1, "worktree": "/somewhere/else", "port": 5343, "started": "x"}\n' > "$STACK_LOCK_ROOT/stack-port-5343.lock/meta.json"
bash "$TD" && test -d "$STACK_LOCK_ROOT/stack-port-5343.lock" && echo "foreign lock untouched OK"
bash "$TD" && echo "double-run idempotent OK"
rm -rf "$STACK_LOCK_ROOT" "$WORK"; unset STACK_LOCK_ROOT
```

Note: in the marker/lock cases `mise run dev:kill` fires — in a mktemp dir it errors and is swallowed by `|| true`; that's the expected standalone-test behavior.

- [ ] **Step 4: Write `prompt.md`**

```markdown
You are running managed-stack e2e tests in a git worktree of {{platform_repo}}. Spawn the stack yourself, test every scenario, tear it all down. You never ask the user anything.

**Args:** post_pr = `{{post_pr}}`. Scenarios (1..N, named):

{{scenarios}}

## Constants

```bash
LOCK_SH={{queohoh_workspace}}/platform/shared/stack-lock.sh
T1_ENV={{platform_repo_path}}.testing1/.env.worktree
PORT=$(sed -n 's/^TRAEFIK_HTTPS_PORT=\([0-9][0-9]*\)$/\1/p' "$T1_ENV" | head -1)
WT=$(pwd)
```

If `T1_ENV` doesn't exist or `PORT` comes out empty, stop: report `🚧 blocked — testing1 worktree env not found at $T1_ENV`.

## Phase 1 — acquire the port lock

```bash
bash "$LOCK_SH" acquire "$PORT" "task:autotest"
```

Exit 1 → the stack is claimed. Report `🚧 blocked`, quoting the stderr owner line verbatim (it names the holder and, when stale, the one-line recovery). Stop — do NOT touch the lock, the env, or any process.

## Phase 2 — port probe

```bash
lsof -nP -iTCP:$PORT -sTCP:LISTEN || true
```

Any listener → someone is running an unmanaged stack on testing1's ports (probably the user's interactive tab). Release the lock (`bash "$LOCK_SH" release "$PORT"`), report `🚧 blocked — unmanaged process on port $PORT; not killing anything I didn't spawn`, and stop.

## Phase 3 — env swap

```bash
if [ -f .env.worktree ]; then cp .env.worktree .env.worktree.autotest-bak; else touch .env.worktree.autotest-absent; fi
cp "$T1_ENV" .env.worktree
```

## Phase 4 — spawn the stack

Start `mise run dev` from the worktree root as a background Bash task (`run_in_background: true`). Then poll readiness, up to 10 minutes:

```bash
for i in $(seq 1 60); do
  code=$(curl -ks -o /dev/null -w '%{http_code}' "https://rate-review.localhost:$PORT/" || true)
  case "$code" in 2*|3*) echo READY; break;; esac
  sleep 10
done
```

No READY within budget → go straight to Phase 6 teardown, then report `🚧 blocked — stack not ready after 10min` with the last ~20 lines of the dev process output as debug signal.

## Phase 5 — run the scenarios

Read the shared testing core:

```bash
cat {{queohoh_workspace}}/platform/shared/autotest-core.md
```

For EACH scenario, in order, dispatch one subagent (Agent tool, `subagent_type: "general-purpose"`, foreground — wait for each before starting the next). The subagent prompt is the core's content with `<SCENARIO>` replaced by that scenario's text and `<CWD>` replaced by `$WT`. Collect each subagent's final `Output:` block and its verdict.

A scenario subagent that errors or returns no parseable verdict counts as `🚧 blocked` for that scenario. Keep going with the remaining scenarios.

## Phase 6 — teardown (ALWAYS, on every path after Phase 3)

```bash
mise run dev:kill
if [ -f .env.worktree.autotest-bak ]; then mv -f .env.worktree.autotest-bak .env.worktree; elif [ -f .env.worktree.autotest-absent ]; then rm -f .env.worktree .env.worktree.autotest-absent; fi
bash "$LOCK_SH" release "$PORT"
```

(The task's post_run hook re-runs an idempotent version of this as a backstop — but do the graceful teardown yourself; the hook is for crashes.)

## Phase 7 — report, and optionally post to the PR

Build the verdict table — one row per scenario: `| <scenario one-liner> | <verdict> |`.

**If post_pr is "true":**

1. `PR=$(gh pr view --json number -q .number)` — no open PR for this branch → note "post_pr requested but no open PR" in the report and skip posting.
2. Filter the results: drop every `🚧 blocked` scenario. Nothing left → post NOTHING (the full table stays in the task report only).
3. Otherwise post ONE comment:

```bash
gh pr comment "$PR" --body "$(cat <<'EOF'
## {{bot_name}} E2E Test Report

*Automated e2e run against a locally spawned stack ({{bot_signature}}).*

| Scenario | Verdict |
|---|---|
<one row per non-blocked scenario>

<for each ❌ failed / ⚠️ partial scenario: its full Output block (Testing steps, Outcome, Debug signals). ✅ works scenarios get their header lines only.>
EOF
)"
```

**Task report (always, your final output):** the FULL verdict table including blocked rows, each scenario's `Output:` block, the screenshot dir path, whether a PR comment was posted (and its URL), and any teardown anomalies.
```

- [ ] **Step 5: Verify the definition parses**

The daemon (restarted in Task 4) must list it: call `mcp__queohoh__list_task_definitions` and confirm `platform/autotest` appears with description, 3 args, and no parse warning in the daemon log.

- [ ] **Step 6: Commit**

```bash
git -C /Users/noootown/workspace add queohoh/platform/tasks/autotest/
git -C /Users/noootown/workspace commit -m "feat(platform): autotest task — managed-stack e2e with lock, env swap, teardown"
```

### Task 8: `tasks/self-review-e2e/`

**Files:**
- Create: `/Users/noootown/workspace/queohoh/platform/tasks/self-review-e2e/config.yaml`
- Create: `/Users/noootown/workspace/queohoh/platform/tasks/self-review-e2e/prompt.md`

**Interfaces:**
- Consumes: `stack-lock.sh`, `autotest-core.md`.
- Produces: nothing consumed downstream — terminal deliverable.

- [ ] **Step 1: Write `config.yaml`**

```yaml
# self-review-e2e — mirror of self-review for e2e strengthening: explore the
# branch's user-visible changes against the USER'S OWN already-running stack
# (never spawns, never swaps env — the author set the stack up the right way
# for their PR), then pin the flows that work with committed Playwright
# specs. Offline: zero GitHub side effects. Failed flows become report
# findings, never specs.
description: E2E-explore this branch's changes against your running stack, then pin working flows with Playwright specs
args:
  - name: target
    type: worktree
    description: worktree to strengthen; inferred/locked when launched from a worktree, else pick/type one
dedup: none # re-runnable — strengthen again after more commits land
worktree: "worktree:{{target}}"
# No verify, same rationale as self-review: this runs in a live WIP worktree
# that legitimately carries uncommitted work, so a porcelain check would
# false-fail; the run's own report is the signal.
model: opus
timeout: 60m
priority: normal
```

- [ ] **Step 2: Write `prompt.md`**

```markdown
You are in a git worktree of {{platform_repo}}, already on the branch to strengthen. This is an OFFLINE run: explore the branch's user-visible changes e2e against the author's own already-running stack, then pin the flows that work with committed Playwright specs. Zero GitHub side effects — no PR comments, no labels, no pushes.

## Autonomy

**You never ask the user a question. Ever.** Make every call yourself, record it as a stated assumption, and proceed. The only permitted halts are the hard aborts named below — those stop with a message; they are not questions.

## Step 0 — lock + stack probe

```bash
LOCK_SH={{queohoh_workspace}}/platform/shared/stack-lock.sh
PORT=$(bash "$LOCK_SH" port)   # THIS worktree's stack port (its own .env.worktree, else base 3443)
bash "$LOCK_SH" acquire "$PORT" self-review-e2e
```

Acquire exits 1 → the stack is in use (e.g. an interactive /autotest run). Report `🚧 blocked`, quoting the stderr owner line, and stop.

```bash
code=$(curl -ks -o /dev/null -w '%{http_code}' "https://rate-review.localhost:$PORT/" || true)
```

Not 2xx/3xx → release the lock and stop: `🚧 blocked — stack unreachable on port $PORT; start your dev stack in this worktree first`. NEVER spawn services and NEVER touch .env.worktree — the author set this stack up the right way for their PR.

**From here on, release the lock (`bash "$LOCK_SH" release "$PORT"`) on EVERY exit path — success, failure, or abort.**

## Step 1 — derive scenarios from the branch diff

```bash
BASE=$(git merge-base HEAD origin/main)
git diff --stat "$BASE"..HEAD
git diff "$BASE"..HEAD
```

Walk the diff once and answer: what does a USER (end-user in the portal, or downstream caller) see differently because of this branch? Then write **at most 2-3 named scenarios** that exercise the change end-to-end, plus at least one adjacent regression risk. Unit tests cover the rest — don't list mechanical coverage. Each scenario is 1-3 sentences of plain-language behavior ("Client admin invites a law firm and verifies the invitation email lands in MailHog"), never "Scenario A"/"Test 1" labels. A branch with no user-visible surface (pure refactor, docs, CI) → release the lock and report "no e2e-testable surface" with the reasoning; that is a valid, successful outcome.

## Step 2 — explore

Read the shared testing core:

```bash
cat {{queohoh_workspace}}/platform/shared/autotest-core.md
```

For EACH scenario, in order, dispatch one subagent (Agent tool, `subagent_type: "general-purpose"`, foreground) with the core's content, substituting `<SCENARIO>` with the scenario text and `<CWD>` with this worktree's absolute path. Collect verdicts (✅ works / ⚠️ partial / ❌ failed / 🚧 blocked).

## Step 3 — pin the working flows with Playwright specs

For each scenario that came back ✅ (and the working portion of a ⚠️), write a Playwright e2e spec that codifies the behavior:

- Rate Review surfaces → `domains/rate-review/apps/portal/` e2e suite; Select surfaces → `domains/select/apps/e2e-tests/`.
- Follow the repo's e2e authoring rules: every spec creates its OWN fixture state (Rate Review has `POST /api/v1/e2e/fixtures/rate-negotiation` for injecting negotiations — never depend on a seed negotiation's mutated state); extend an existing spec file when the surface already has one; use the existing page objects.
- Run ONLY the new/changed spec file(s) locally against the running stack (few at a time — never the whole suite), and iterate until green.

Scenarios that ❌ failed are findings for the author — record exactly what broke (page, error, response code) in the report. NEVER commit a spec that codifies broken behavior, and never mark specs skipped as TODO pins.

## Step 4 — commit

**This is a live WIP worktree.** Stage and commit ONLY the spec/page-object files you created or edited, by explicit path. Never `git add -A`, never touch or revert pre-existing dirty files — the author's unrelated uncommitted work lives here and is not yours. Commit message: `test(e2e): pin <short description> flows`. Do not push.

## Step 5 — release the lock and report

```bash
bash "$LOCK_SH" release "$PORT"
```

End with exactly this report block, nothing after it:

```
Self-review-e2e — <N flows pinned | no e2e-testable surface | blocked>
| Scenario | Verdict |
|---|---|
<one row per scenario>

Committed specs: <paths + commit sha, or "none">
Findings: <for each ❌/⚠️: 1-3 sentences on what broke — or "none">
Screenshots: <CWD>/.agents/screenshots/
```
```

- [ ] **Step 3: Verify the definition parses**

`mcp__queohoh__list_task_definitions` → `platform/self-review-e2e` listed, no daemon-log parse warning.

- [ ] **Step 4: Commit**

```bash
git -C /Users/noootown/workspace add queohoh/platform/tasks/self-review-e2e/
git -C /Users/noootown/workspace commit -m "feat(platform): self-review-e2e task — offline e2e strengthening"
```

### Task 9: extend `pr-review`

**Files:**
- Modify: `/Users/noootown/workspace/queohoh/platform/tasks/pr-review/config.yaml`
- Modify: `/Users/noootown/workspace/queohoh/platform/tasks/pr-review/prompt.md`

**Interfaces:**
- Consumes: the `autotest` definition contract (Task 7): `run_task_definition {repo: "platform", name: "autotest", args: [<target>, <scenarios>, "true"], cwd: <pwd>}`.

- [ ] **Step 1: Add the two args to `config.yaml`**

After the existing `target` arg entry, add:

```yaml
  - name: static_review
    default: "false"
    options: ["false", "true"]
    description: run the static review path (size-based rules, specialist team on large PRs)
  - name: e2e_review
    default: "false"
    options: ["false", "true"]
    description: run /pr-tldr and enqueue an autotest e2e run that posts its own PR comment
```

Also update the header comment block: note that both modes default off, so a manual run must opt in, and a re-armed cron would early-terminate on every item until scheduled-path defaults are decided (deliberate; cron is currently disabled).

- [ ] **Step 2: Add the mode gate to `prompt.md`**

Immediately after the `$PR` detection paragraph (before "**Title:**"), insert:

```markdown
## Mode gate

Requested modes: static_review = `{{static_review}}`, e2e_review = `{{e2e_review}}`.

- Both `"false"` → respond with exactly `NO_ACTION — no review mode selected` and stop. Do not fetch, classify, or read anything further.
- Run the **Static review** path (Setup → Review → After Review below) only when static_review is `"true"`.
- Run the **E2E review** section only when e2e_review is `"true"`. When both are `"true"`, do the static path first, then e2e.
```

- [ ] **Step 3: Append the e2e section to `prompt.md`**

After the "After Review" section (before "## Output Format"), insert:

```markdown
## E2E review (only when e2e_review is "true")

1. Invoke the `pr-tldr` skill via the Skill tool: `skill: "pr-tldr"`, `args: "$PR"`. It is read-only and produces, among other sections, 2-3 named smoke-test scenarios.
2. Extract the scenario texts from its smoke-test section — the named scenarios with their step descriptions, NOT the prerequisites block (the autotest task manages its own stack). If pr-tldr surfaces no user-visible scenario (pure refactor/docs PR), skip the enqueue and state that in your output; that is a valid outcome.
3. Enqueue the autotest task via the queohoh MCP:
   - Tool: `mcp__queohoh__run_task_definition`
   - Params: `repo: "platform"`, `name: "autotest"`, `args: [<current worktree name>, <the extracted scenarios, verbatim, as one text block>, "true"]`, `cwd: <absolute path of this worktree (pwd)>`.
   - `cwd` pins the run to this same PR worktree; the `"true"` is `post_pr`, so the autotest task will post its own separate e2e comment when it finishes (blocked-only results post nothing).
4. Note the created task id in your output. Do NOT wait for or poll the task.
```

- [ ] **Step 4: Update the Output Format section**

Add two bullets to "## Output Format":

```markdown
- **If both modes were "false":** your entire output is `NO_ACTION — no review mode selected`.
- **If e2e_review ran:** append a line `E2E: enqueued autotest task <id> (post_pr=true)` — or `E2E: skipped, no testable scenario` — after the static output (or as the whole output when static_review was "false").
```

- [ ] **Step 5: Verify parse + gate behavior**

`mcp__queohoh__list_task_definitions` → `platform/pr-review` now shows 3 args. Then dry-run the gate: `mcp__queohoh__run_task_definition {repo: "platform", name: "pr-review", args: ["<any existing worktree>"], cwd: ...}` with defaults and confirm the run's report is exactly `NO_ACTION — no review mode selected` (cheap — the gate stops before any fetch).

- [ ] **Step 6: Commit**

```bash
git -C /Users/noootown/workspace add queohoh/platform/tasks/pr-review/
git -C /Users/noootown/workspace commit -m "feat(platform): pr-review static_review/e2e_review opt-in modes"
```

---

## Phase C — `/autotest` skill (cwd: `/Users/noootown/workspace/claude-code/skills/autotest`)

### Task 10: skill refactor — port lock + shared core

**Files:**
- Modify: `/Users/noootown/workspace/claude-code/skills/autotest/SKILL.md`

**Interfaces:**
- Consumes: `stack-lock.sh` CLI, `autotest-core.md`.

- [ ] **Step 1: Add the lock to Step 1 (sanity gate)**

After the existing two gate checks (non-empty scenario, platform-repo cwd) and the `<CWD>` capture, add a third gate:

```markdown
3. **Acquire the stack lock.** The skill tests whatever stack this worktree points at; that stack must not be mid-use by a queohoh autotest/self-review-e2e run (which may be running DIFFERENT code on the same ports).

   ```bash
   LOCK_SH=/Users/noootown/workspace/queohoh/platform/shared/stack-lock.sh
   PORT=$(bash "$LOCK_SH" port)
   bash "$LOCK_SH" acquire "$PORT" interactive
   ```

   If acquire exits 1, print the stderr owner line verbatim plus:
   > `/autotest: stack on port <PORT> is locked by another e2e run — wait for it or (if the message says stale) rm -rf the named lock dir.`
   and stop. If it succeeds, remember `<PORT>` — it is substituted into the subagent prompt (the subagent releases the lock when it finishes).
```

- [ ] **Step 2: Replace the inline subagent template with the shared core**

In "## Step 2 — Dispatch", change the prompt-construction instruction to:

```markdown
- `prompt`: build it as three parts, in order:
  1. The full content of `/Users/noootown/workspace/queohoh/platform/shared/autotest-core.md` (read it with the Read tool), with `<SCENARIO>` replaced by the scenario text and `<CWD>` replaced by the captured working directory.
  2. This lock-release footer, with `<PORT>` substituted:

     ```
     ## Lock release (ALWAYS, last action before your final message)

     bash /Users/noootown/workspace/queohoh/platform/shared/stack-lock.sh release <PORT>

     Release on EVERY path — success, failure, blocked, or timeout. The lock was acquired by the dispatching session on your behalf.
     ```
```

Then DELETE the entire "## Subagent prompt template" section (the fenced template body) — it now lives in autotest-core.md. Keep Steps 3 and 4 (fire-and-forget + verbatim passthrough) unchanged.

- [ ] **Step 3: Update the frontmatter description**

Replace the frontmatter `description` sentence "Services must already be running." with "Services must already be running; acquires the port-keyed stack lock for the run (aborts if another e2e run holds it)."

- [ ] **Step 4: Smoke-test the gates manually**

- From a platform worktree with no lock held: run `/autotest "dummy"` far enough to see `Spawning autotest subagent…` (then cancel the subagent, and verify the lock dir got created — then confirm it's released after the subagent ends or clean it manually).
- Pre-create a lock (`bash $LOCK_SH acquire <port> test-owner` from that worktree), run `/autotest "dummy"`, and verify the hard abort message.
- `rm -rf` the test lock afterwards.

- [ ] **Step 5: Commit**

```bash
git -C /Users/noootown/workspace add claude-code/skills/autotest/SKILL.md
git -C /Users/noootown/workspace commit -m "refactor(autotest-skill): consume shared autotest-core + port-keyed stack lock"
```

---

## Phase D — end-to-end validation

### Task 11: validation runs

No new files — this task exercises the built system and fixes anything it surfaces.

- [ ] **Step 1: Lane serialization proof**

Enqueue two `autotest` runs against two different small branches/worktrees back-to-back (scenarios can be a trivial "open the portal landing page and verify it renders" one-liner; `post_pr` default false). Watch `mcp__queohoh__list_tasks` (or the TUI): the second must stay `queued` until the first reaches a terminal status, then run. Both must end `done` with a verdict table in their reports.

- [ ] **Step 2: Full managed-lifecycle audit of one run**

During/after one of the Step 1 runs verify, in order: lock dir exists during the run (`bash stack-lock.sh status 4343` → owner `task:autotest`); `.env.worktree` in the target worktree equals testing1's during the run; stack answers on `https://rate-review.localhost:4343`; after the run — lock free, env restored (or removed), `lsof -nP -iTCP:4343 -sTCP:LISTEN` empty, screenshots present under the worktree's `.agents/screenshots/`.

- [ ] **Step 3: Port-conflict abort**

Start a manual stack that binds 4343 (or fake it: `python3 -m http.server 4343` in another tab), enqueue an autotest run, and verify it lands `done` with a `🚧 blocked — unmanaged process on port 4343` report, the lock free afterwards, and the fake server untouched. Kill the fake server.

- [ ] **Step 4: pr-review modes**

On a real small PR worktree: run pr-review with defaults → report is exactly `NO_ACTION — no review mode selected`. Run with `args: [<target>, "false", "true"]` (e2e only) → verify pr-tldr ran, an autotest task was enqueued with `post_pr=true`, and when it finishes the PR shows ONE e2e comment with no 🚧 rows (or no comment if all-blocked).

- [ ] **Step 5: self-review-e2e on a WIP branch**

With a dev stack running in one of your own worktrees, run self-review-e2e against it. Verify: lock held during the run and released after; scenarios derived from the diff; specs committed by explicit path only (check `git log --stat -1` shows only spec/page-object files); the report block matches the template; no GitHub side effects (`gh pr view --comments` unchanged if a PR exists).

- [ ] **Step 6: Fix-forward and commit anything the validation surfaced**

Any prompt-wording, polling-budget, or teardown fix goes in as its own small commit in the owning repo.
