# @queohoh/core — agent guide

Pure domain logic for the queohoh task queue: the task model, the queue store, the scheduler, definition→task instantiation, and the single run lifecycle. No process/daemon orchestration lives here — that's `@queohoh/daemon`, which wires this package's seams to a running process.

**All IO is injected**, never reached for directly, so the logic stays testable: `Exec` (shell), `ResolverIO` (git/worktree ops), `ClaudeExecutor` (spawns claude). The ONE exception is `runner.ts`, which owns the actual child spawn.

## Hierarchy — what owns what

```
src/
  task.ts            TaskInstance + zod schema (status lifecycle, chain fields);
                     parse/serialize task files; laneKey(). Source of truth for
                     the on-disk task shape.
  store.ts           QueueStore: task-file CRUD, create()/createChain(), archive,
                     listArchived, finishedAt stamping. Monotonic ulid ids.
  scheduler.ts       schedule(): PURE decision fn → {start, resolve, skip}. All
                     gating + chain ordering. No side effects.
  worker.ts          runTask(): the full single-run lifecycle (resolve cwd →
                     render → pre_run → claude → dirty-tree check → verify
                     (done-condition gate) → post_run → terminal status). Reads
                     injected deps only.
  runner.ts          executeClaude() + executeVerify(): the ONLY process spawns
                     in core. executeClaude: stream-json parse,
                     timeout→SIGTERM→SIGKILL, returns RunResult. executeVerify:
                     `/bin/bash -lc` the verify cmd, same kill escalation,
                     tail-bounded combined output → VerifyResult.
  resolver.ts        TargetRef→worktree resolution (resolveTarget), WorktreeInfo,
                     ResolverIO interface, REPO_SENTINEL.
  resolver-io.ts     Concrete ResolverIO (git/`wt` shell) + defaultExec (Exec).
  ref.ts             TargetRef grammar: parseRef/formatRef/extractRef/
                     extractTicketId. Paste-friendly URL forms live here.
  instantiate.ts     Definition→task(s): instantiateDefinition, buildItemFromArgs,
                     refOverride + `worktree: auto` extraction, dedup wiring.
  definition.ts      TaskDefinition schema + loader (config.yaml + prompt.md).
  config.ts          Global config.yaml + per-project vars.yaml loaders; reserved
                     keys; resolveDefinition (project → global fallback).
  models.ts          Model-alias table (DEFAULT_MODEL_ALIASES, resolveModel,
                     effectiveModelTable). Unknown names pass through.
  sessions.ts        SessionRegistry (interactive/worker sessions) + buildLiveState.
  main-sessions.ts   MainSessionStore: lane → session-id pointer for main/resume
                     chaining (a follow-up run resumes the previous run's session).
  run-store.ts       Per-run artifacts on disk (see "run-store" below).
  dedup.ts           filterNewItems (discovery dedup modes).
  discovery.ts       Run a definition's discovery command → items.
  template.ts        render(): {{var}} substitution; UNKNOWN keys stay literal.
  frontmatter.ts     YAML-frontmatter parse/stringify for task files.
  hooks.ts           execHook: run a pre_run/post_run shell command.
  redact.ts          Redactor: secret redaction for logs/transcripts.
  slug.ts            qooTempName + slug helpers (temp worktree names).
  worktree-context.ts extractTicket + exec-time worktree vars.
  duration.ts        Duration formatting.
  index.ts           Public barrel — the daemon imports only from here.
```

## Decisions (the why)

- **Scheduler gates, and what deliberately does NOT gate.** A lane (`repo:worktree`) serializes: at most one running task per lane, at most one START per lane per tick, under a global `maxConcurrent` cap. A **failed** task no longer pauses its lane, and an **interactive/main session** no longer holds its lane — both were removed by user decision (independent work must not be blocked by an unrelated failure or an open editor). Don't reintroduce lane pausing on failure.
- **`schedule()` is pure.** It returns decisions (`start`/`resolve`/`skip`); the engine performs the writes. Keep side effects out — skip carries a `reason` string the engine stamps.
- **Chains** (`chainId` + `chainSeq`, head = seq 0). The chain resolves its worktree ONCE (at the head); tails inherit it and must never re-resolve (a `temp` chain would otherwise spawn N worktrees). A tail is eligible only when its predecessor is `done`; a predecessor in `failed`/`skipped`/`cancelled`/ `needs-input` (or missing) skips the tail (cascades in one pass).
- **`skipped` vs `cancelled` are distinct and must stay so.** `skipped` = a chain member that never ran because its predecessor didn't succeed. `cancelled` = a human stopped it (stop/skip RPC). Never collapse them into `failed`.
- **Status lifecycle.** Non-terminal: `queued`, `needs-input`, `running`. Terminal: `done`, `failed`, `cancelled`, `skipped`, `verify-failed` (worker claimed success but the `verify` done-condition disagreed — see worker.ts). `store.update` stamps `finishedAt` on ANY terminal transition (all five) and clears it on a re-run.
- **TargetRef resolution rules** (`resolveTarget`): `worktree:<name>` never spawns (must already exist, else needs-input); `ticket:<id>` and `temp` spawn; `pr:<N>` reuses a matching branch/worktree or spawns one; the `@repo` sentinel resolves to the project's primary checkout and never spawns.
- **ref precedence** (in `instantiate`/daemon): `cwd > worktree > ref > definition's worktree:`. `refOverride` always wins over a `worktree: auto` def, which extracts the first PR/ticket URL from arg values — the escape hatch for when such a URL is reference material, not the destination.
- **vars.yaml reserved keys.** `models:` and `github_id:` are read as SETTINGS (via `loadProjectModels`/`loadProjectGithubId`), not exposed as `{{template}}` vars. Every other key becomes a template var.
- **Model aliases pass through.** `resolveModel` returns unknown names (incl. full model ids) unchanged, so a concrete id always works.
- **`render` leaves unknown placeholders literal**, enabling a two-pass model: instantiate-time fills project/repo/item vars; the worker's exec-time pass fills `{{worktree}}`/`{{branch}}`/`{{ticket}}`.

## run-store — the TUI reads these files directly

`RunStore` writes per-run artifacts the TUI consumes without going through the socket: `events.jsonl` (raw stream), `transcript.md` (rendered), `data.json` (`resolved_worktree`, `session_id`, outcome, reason, timings), `worker.json` (pid). Field names in `data.json` are snake_case and load-bearing for the TUI — do not rename them.

## Conventions (do this)

- **Task fields are additive-only.** New `TaskInstance` fields must be optional with a zod `.default(null)` so legacy task files still parse. The zod schema is `.strict()` — a typo'd key throws by design.
- **Adding a TaskStatus is a SWEEP.** Update, in lockstep: the zod enum (`task.ts`), scheduler terminal checks (`isTerminalNonSuccess` if it blocks a chain tail), `store.update` finishedAt stamping, the worker `outcome` union + `run-store.finishRun` outcome type, and the daemon's auto-archive rule. Missing one silently mis-handles the new status.
- **Never spawn processes outside `runner.ts`.** Everything else takes `Exec`/ `ResolverIO`/`ClaudeExecutor` as an injected dep.
- **Import the daemon-facing surface from `index.ts` only.** If it isn't exported there, it isn't public.
