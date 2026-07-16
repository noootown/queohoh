# Autotest platform task, pr-review e2e integration, self-review-e2e — design

Date: 2026-07-16
Status: approved pending user review

## Problem

The interactive `/autotest` skill tests one scenario against an already-running local stack. Using it today requires manual ceremony: copy `../platform.testing1/.env.worktree` into the PR worktree, run `mise run dev` in a second tab (often forgotten), run `/pr-tldr` for scenarios, then feed them to `/autotest`. There is no queohoh task form of this flow, no concurrency protection around the single shared testing1 port/credential set, and `pr-review` only does static review.

## Goals

1. A queohoh `autotest` platform task that owns the full stack lifecycle (spawn `mise run dev` with testing1's env, test, tear down) so nothing must be pre-started.
2. Keep the interactive `/autotest` skill for in-session use (still assumes services running), hardened with a mutex precheck.
3. Hard concurrency 1 for anything that spawns the testing1-env stack: a daemon-level lane override plus a filesystem mutex.
4. Extend `pr-review` with two opt-in booleans: `static_review` (existing agent-team path) and `e2e_review` (pr-tldr → autotest handoff). Both default false; both false → early terminate; both true → two separate PR comments.
5. E2e PR comments include only ❌ failed / ⚠️ partial / ✅ works verdicts; 🚧 blocked stays local-only.
6. A `self-review-e2e` task mirroring `self-review`: offline PR-strengthening — explore the branch's changes e2e against the user's own already-running stack, then pin working flows with committed Playwright specs. Zero GitHub side effects, no stack spawn.

## Architecture overview

Three repos, six deliverables:

| # | Deliverable | Repo |
|---|---|---|
| 1 | `lane:` override field on task definitions | daemon source (`~/Downloads/agent247/queohoh`) |
| 2 | `platform/shared/autotest-core.md` — shared testing brains | config repo (`~/workspace/queohoh`) |
| 3 | `tasks/autotest/` — managed-stack e2e task (+ `teardown.sh`) | config repo |
| 4 | `pr-review` extension (`static_review`, `e2e_review`) | config repo |
| 5 | `tasks/self-review-e2e/` — offline e2e strengthening task | config repo |
| 6 | `/autotest` skill update (mutex precheck + shared-core refactor) | skills repo (`~/workspace/claude-code/skills/autotest`) |

The split of responsibilities: **autotest-core.md** holds only the testing brains (credentials table, agent-browser conventions, screenshot rules, per-scenario verdict/report template) and assumes a reachable stack. **Stack lifecycle** (mutex, env copy, spawn, readiness, teardown) lives exclusively in the `autotest` task prompt + `teardown.sh`. This lets three consumers share the core in different modes: the autotest task (managed stack), the `/autotest` skill (user's stack), and `self-review-e2e` (user's stack).

## 1. Daemon: `lane:` override

Today lane = `repo:worktree`, and concurrency is per-project (`max_concurrent_tasks`) plus per-lane serialization. Autotest instances run in *different* PR worktrees, so the scheduler cannot serialize them.

Change:

- `DefinitionConfigSchema` (packages/core/src/definition.ts) gains optional `lane: z.string().min(1).optional()`; carried through `TaskDefinition`, instantiate, and persisted on `TaskInstance` (so serialization survives daemon restarts).
- `laneKey(task)` (packages/core/src/task.ts): **the override applies only after worktree resolution.** `worktree === null` still returns `null` (preserving the scheduler's resolve path — a pre-resolve non-null lane would send an unresolved task straight to `startWorker`). Once resolved, an override returns `${repo}:${lane}` instead of `${repo}:${worktree}`.
- `buildLiveState` picks the override up automatically wherever it derives running lanes from `laneKey(task)`; interactive-session lane mapping (cwd-based) is unaffected — interactive sessions don't hold lanes anyway.
- Tests: two queued lane-override tasks in different worktrees → one starts, one waits; override task + normal tasks → no interference; restart persistence.

No TUI changes — lanes are not surfaced there today.

## 2. `platform/shared/autotest-core.md`

Extracted from the `/autotest` skill's subagent prompt body:

- Credentials table (Rate Review WorkOS/Legacy client/Legacy firm, Select WorkOS) with state-file paths and auth flow (load → verify → fallback login → save).
- agent-browser conventions (headed sessions, timestamped session names).
- Screenshot rules (`<worktree>/.agents/screenshots/`, run-slug filenames, never delete prior runs').
- Per-scenario budget (8 min) and the verdict vocabulary: ✅ works / ⚠️ partial / ❌ failed / 🚧 blocked.
- Per-scenario report template; multi-scenario runs emit a verdict table.

The core assumes the stack is reachable and says nothing about spawning or teardown. Consumers `cat` it (task prompts via `{{queohoh_workspace}}`, the skill via the absolute path).

## 3. `autotest` platform task

### Definition (config.yaml sketch)

```yaml
description: E2E-test scenarios in the target worktree against a self-spawned stack (testing1 env)
args:
  - name: target
    type: worktree        # inferred/locked when launched from a worktree row or /qoo
  - name: scenarios
    type: text            # 1..N named scenarios, typically pr-tldr output
  - name: post_pr
    default: "false"
    options: ["false", "true"]
lane: testing1-stack      # new daemon field — all instances serialize
worktree: "worktree:{{target}}"   # fallback only; /qoo and TUI pass a ref override
dedup: none
post_run: bash {{queohoh_workspace}}/platform/tasks/autotest/teardown.sh
model: opus
timeout: 60m
priority: normal
```

### Mutex

Portable mkdir-lock (macOS has no flock binary) at `~/workspace/queohoh/platform/state/testing1-stack.lock/` containing `meta.json`: `{owner: "<task-id>" | "interactive", pid, worktree, started}`. Acquire = atomic `mkdir`. Held → abort `🚧 blocked`, reporting the owner. Dead owner PID → still abort (never auto-reclaim), but the message names it stale and gives the one-line `rm -rf` recovery.

### Run sequence

1. **Acquire mutex** (abort blocked if held).
2. **Port probe** testing1's portal port. Occupied → release mutex, abort blocked: "unmanaged process on port N — probably your interactive stack". The task never kills processes it didn't spawn.
3. **Env swap**: back up any existing `.env.worktree` to `.env.worktree.autotest-bak`, then `cp ../platform.testing1/.env.worktree .` (mirrors the manual flow — PR code on testing1 ports/credentials).
4. **Spawn** `mise run dev` in the background from the worktree; poll the portal URL until HTTP 200, budget ~10 min. Not ready → teardown, abort blocked.
5. **Test**: for each scenario in `scenarios`, run a subagent built from `autotest-core.md` (one stack lifecycle amortized across all scenarios). Collect per-scenario verdicts + screenshots.
6. **Teardown (graceful)**: kill the overmind stack it spawned (scoped to this worktree's sockets), restore/remove `.env.worktree`, release mutex.
7. **Report**: full per-scenario verdict table (including blocked) in the task report. If `post_pr=true`: filter out 🚧 blocked rows; if any remain, post ONE PR comment with the surviving verdicts; if all scenarios blocked, post nothing.

### `teardown.sh` backstop

`post_run` hook (runs with cwd = the worktree, even when the worker failed or timed out). Idempotent:

- Only acts when the lock's `meta.json` `worktree` field matches the hook's cwd (hooks run with cwd = the resolved worktree, so the key is always available; a lock owned by a different worktree's run — or by `interactive` — is left alone).
- Kills the spawned stack scoped to this worktree (overmind socket(s) in the worktree + orphan reap by worktree path, per the KB's dev:kill/orphan patterns).
- Restores `.env.worktree.autotest-bak` (or removes the copied file if no backup existed).
- Removes the lock dir.

A crashed or timed-out worker therefore never strands the ports, the env file, or the mutex.

### Side-effect contract

`post_pr` defaults `"false"`: queueing autotest against your own branch never touches GitHub. Only `pr-review` (or an explicit manual launch) passes `post_pr=true`.

## 4. `pr-review` extension

New args, both default `"false"`, manual opt-in:

```yaml
  - name: static_review
    default: "false"
    options: ["false", "true"]
  - name: e2e_review
    default: "false"
    options: ["false", "true"]
```

Prompt gate before classification: both false → output `NO_ACTION — no review mode selected`, stop.

- `static_review=true` → the existing path verbatim: size-based rules (small-bugfix skip / `/review` light / review-core full) and the existing comment-review posting.
- `e2e_review=true` → invoke the `/pr-tldr` skill on the PR (headless workers inherit the global skill + MCP config), extract its named scenarios, then `mcp__queohoh__enqueue_task` an `autotest` instance with the PR worktree ref override, `scenarios=<extracted text>`, `post_pr="true"`. pr-review then finishes; the e2e comment lands later from the autotest task. If pr-tldr yields no testable scenario (pure refactor/docs PR), skip the enqueue and say so in the pr-review output.
- Both true → static comment from pr-review + e2e comment from autotest = two separate PR comments, as required.

**Cron note**: with both defaults false, a re-armed cron/discovery pr-review early-terminates on every item. Cron is currently disabled so this costs nothing now; re-arming later requires choosing scheduled-path defaults (discovery items don't read declared args — solvable then, out of scope here).

## 5. `self-review-e2e` platform task

Mirrors `self-review`'s conventions: `target` (type worktree, inferred), `worktree: "worktree:{{target}}"`, `dedup: none`, no `verify` (live WIP worktree — same rationale as self-review's comment block), `model: opus`, `timeout: 60m`, full-autonomy header (never asks questions).

Flow:

1. **Derive scenarios from the merge-base diff** (no PR required): a condensed pr-tldr-style step embedded in the prompt — walk the diff, identify what a user sees differently, emit 2–3 named scenarios incl. one adjacent regression risk. (Deliberately not shared with the pr-tldr skill: the skill is PR-mechanics-shaped; sharing would be premature.)
2. **Probe the user's stack** (portal URL from the worktree's own env/ports). Unreachable → abort `blocked` with "start your stack first". Never spawns, never copies env — the user set this stack up correctly for their PR.
3. **Explore**: run the scenarios via `autotest-core.md`, collect verdicts.
4. **Pin working flows**: author Playwright e2e specs for the behaviors that passed, following the repo's e2e authoring rules (specs create their own fixture state; extend existing spec files where natural; run only the new specs locally, iterate to green against the running stack). Failed scenarios are NOT codified — they go in the report as findings.
5. **Commit only its own spec files by explicit path** (never `git add -A`; the WIP worktree's unrelated dirty files are untouched).
6. **Report locally**: scenario verdict table + list of committed specs + findings for failed flows. Zero GitHub side effects.

No mutex, no `lane` override: it uses the user's own stack, not the shared testing1 resource.

## 6. `/autotest` skill update

Kept for interactive use; two changes:

- **Mutex precheck (check-only, hard abort)**: sanity gate reads the lock dir; if held, print the owner (`meta.json`) and stop. The skill does NOT acquire the lock — it tests against the user's own already-running stack, and holding it for a whole interactive session would starve queued tasks. This closes the wrong-code window (a queohoh autotest stack up on testing1 ports while you test interactively).
- **Shared-core refactor**: the subagent prompt body becomes `cat /Users/noootown/workspace/queohoh/platform/shared/autotest-core.md` + the skill-specific wrapper (sanity gate, fire-and-forget dispatch, verbatim passthrough — unchanged).

## Verdict and posting rules (summary)

| Verdict | Task report | PR comment (post_pr=true) | self-review-e2e |
|---|---|---|---|
| ✅ works | yes | yes | pinned with a Playwright spec |
| ⚠️ partial | yes | yes | reported; spec only for the working part if clean |
| ❌ failed | yes | yes | reported as a finding, never codified |
| 🚧 blocked | yes (local only) | filtered out; all-blocked → no comment at all | reported |

## Known risks and accepted limitations

- **localhost cookie clobber**: agent-browser state files are shared and localhost cookies are host-only (port-blind — see KB `select_multiport-cookie-clobber`). The mutex + lane serialize all managed-stack runs, but a `self-review-e2e` run concurrent with an autotest task run can clobber auth state mid-run. The core's re-login fallback recovers; accepted, not serialized.
- **Stale lock is manual-recovery by design**: never auto-reclaimed, one `rm -rf` documented in the abort message.
- **`.env.worktree` mutation window**: between env swap and teardown the PR worktree carries testing1's env. The backup/restore in both graceful teardown and `teardown.sh` bounds this to the run; a hard daemon kill before `post_run` fires would leave the swap in place (visible, recoverable from `.autotest-bak`).
- **pr-review cron becomes a no-op** with both defaults false (documented above; deliberate).
- **Seed/DB state is out of scope**: the managed stack uses whatever DB state the testing1 env points at; scenarios that need special seeding will land `blocked`, which is the correct signal.

## Testing strategy

- **Daemon**: unit tests in `scheduler`/`task`/`definition` suites for the lane override (serialization, resolve-path preservation, persistence, no cross-def interference).
- **Config-repo tasks**: exercised by real runs — first an `autotest` run against a known-good small PR (verifying spawn/teardown/lock lifecycle and a clean abort when a manual stack holds the ports), then a `pr-review` run with each boolean combination, then a `self-review-e2e` run on a WIP branch.
- **teardown.sh**: runnable standalone; verify idempotence (double-run), no-lock no-op, and wrong-owner no-op.
- **Skill**: manual — run `/autotest` with and without a held lock.

## Out of scope

- Per-definition `concurrency: N` (lane override covers the need).
- Scheduled/cron defaults for pr-review's new booleans.
- Auto-reclaim of stale locks.
- Sharing the scenario-derivation logic between pr-tldr and self-review-e2e.
- Seeding/fixture management for the managed stack.
