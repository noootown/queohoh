# Cron scheduler (slice 2) — design

## Problem

Task definitions carry a `cron:` field (`packages/core/src/definition.ts`), and the
daemon parses and displays it — but nothing fires it. The field is DISPLAY-ONLY:
`pass()` in `engine.ts` never evaluates schedules. A definition with `cron: "0 * * * *"`
never runs on its own; it only runs when a human triggers `runDefinition` via the TUI
or MCP.

Slice 2 makes the daemon fire a definition when its `cron` expression comes due.

## Scope

**In:** time-based firing of any definition with a `cron:` string, for both
discovery-backed defs (e.g. `pr-review`) and discovery-less defs (e.g.
`slack-react-release-notes`).

**Out:** persisting the fire cursor across daemon restarts; per-definition
enable/disable UI; sub-minute schedules; timezone overrides (schedules are local
time); backfilling every missed slot after long downtime (see Catch-up policy).

## Architecture

Follows the repo's pure-core / effectful-daemon split, mirroring the existing lane
`scheduler.ts` (pure decision) ↔ `engine.ts` (performs writes).

```
packages/core/src/cron.ts        PURE. 5-field cron parse + match. No I/O.
  parseCron(expr) -> CronSpec | throw
  cronMatches(spec, date) -> boolean            // does `date` (local) satisfy the spec, to the minute
  cronDue(spec, lastCheckedMs, nowMs) -> boolean // did a scheduled minute fall in (lastChecked, now]

packages/daemon/src/engine.ts    EFFECTFUL. New private evaluateCrons() called from pass().
  - in-memory Map<definition, lastCheckedMs> cursor
  - per-definition in-flight guard (Set<definition>)
  - fire-and-forget instantiateDefinition(..., source: "cron")
```

No new dependency: the cron parser is hand-rolled, consistent with the repo's
no-new-deps convention (clipboard is hand-rolled OSC 52; URL open shells to `open`).

### Component 1 — `packages/core/src/cron.ts` (pure)

Standard Vixie-style 5-field cron: `minute hour day-of-month month day-of-week`.

- Fields support: `*`, a single number, comma lists (`1,15,30`), ranges (`1-5`),
  and steps on `*` or a range (`*/15`, `0-30/10`). Names for months/weekdays are
  **not** supported in slice 2 (numbers only) — neither current cron needs them;
  a name throws a parse error rather than silently mis-scheduling.
- Ranges (numeric): minute 0–59, hour 0–23, dom 1–31, month 1–12, dow 0–6 (0 = Sunday;
  `7` also accepted as Sunday and normalized to 0).
- **dom/dow OR-semantics:** when *both* day-of-month and day-of-week are restricted
  (neither is `*`), a date matches if it satisfies *either* — standard cron behavior.
  When one is `*`, only the other constrains.
- Evaluation is in **local time** (`Date` getters, not UTC): `30 15 * * *` means 15:30
  in the operator's timezone, matching the value migrated from agent247.
- `parseCron` throws on malformed input (wrong field count, out-of-range, unsupported
  token). The caller (definition load already validated non-empty; the engine)
  logs and skips a def whose cron fails to parse, rather than crashing the tick.

`cronMatches(spec, date)` is true iff the local minute represented by `date` satisfies
all fields (with dom/dow OR-semantics). Second/millisecond components are ignored.

`cronDue(spec, lastCheckedMs, nowMs)` returns true iff at least one whole minute
`m` with `lastCheckedMs < m <= nowMs` satisfies `cronMatches`. Implementation walks
minute boundaries in `(lastChecked, now]` and returns true on the first match. The
window is clamped to a sane maximum look-back (e.g. 48h) so a cursor that is somehow
far in the past can never make the tick loop for an unbounded number of minutes; a
match anywhere in the clamped window still fires exactly once (catch-up-once).

### Component 2 — `engine.evaluateCrons()` (effectful)

Called once per `pass()`, after `registry.sweep()` and before the lane `schedule()`
call, so a freshly-enqueued cron task is eligible to start on the *next* tick.

State (engine instance fields, in-memory only):
- `cronCursor: Map<string, number>` — `"repo/name"` → epoch ms of last evaluation.
- `cronInFlight: Set<string>` — definitions whose async fire has not yet settled.

Algorithm per tick:
1. Enumerate every definition across all projects that has a non-null `cron`
   (reuse the same `resolveDefinition`/`listDefinitions` path the API uses).
2. For each def key `repo/name`:
   - If not in `cronCursor`: **seed** `cronCursor[key] = now` and continue (no fire).
     This is the boot / hot-reload safety — the daemon self-restarts on rebuild
     (`reload.ts`), and firing on boot would fire every cron on every code change.
   - If `cronInFlight` has the key: skip (a prior fire is still running discovery).
   - Parse the cron (skip + log on parse error). If `cronDue(spec, cursor, now)`:
     - Advance `cronCursor[key] = now` **immediately** (before the async fire) so a
       slow discovery cannot double-fire on the next tick.
     - Add key to `cronInFlight`; fire-and-forget `fireCron(def)`; on settle
       (success or failure) remove the key from `cronInFlight`.
3. Definitions that vanished from config are pruned from the cursor map so a
   re-added def re-seeds (no stale catch-up).

`fireCron(def)` mirrors the `runDefinition` API path:
- `instantiateDefinition(def, trigger, { store, exec: defaultExec, cwd: projectDir,
  source: "cron", globalVars: {project, repo_path, ...config.vars}, repoVars })`.
- Trigger: `{ mode: "cron" }` (new). In `instantiate.ts`, cron mode with discovery runs
  discovery (identical to `discover` mode); cron mode with no discovery builds a single
  item from arg defaults (all args must have defaults or the def is misconfigured for
  cron — validated: a cron def with a required arg logs and is skipped).
- Errors are swallowed to a `console.error` (never throw into the tick), like the
  git-enrichment path. `onChange()` fires after a successful enqueue so the TUI updates.

Fire-and-forget keeps `pass()` latency-free: the cheap in-memory `cronDue` check runs
every 2s tick, but the expensive discovery shell-out only runs when a slot is actually
due (hourly / daily), and even then off the pass.

### Component 3 — dedup interaction (`instantiate.ts` / `dedup.ts`)

Two independent dedup concerns:
- **Fire-timing dedup** (don't fire the same slot twice): owned by `cronCursor`.
  Authoritative and sufficient.
- **Item dedup** (don't re-review the same PR): the definition's `dedup` field,
  applied by `filterNewItems` over discovered items. Unchanged for discovery defs —
  `pr-review`'s hourly fire re-runs discovery and `skip_seen` skips PRs already queued.

Footgun fix for discovery-less crons: a discovery-less cron fire always yields the
same item(s) — built from arg defaults (or the static `adhoc` key when there are no
args, via `defaultKeyTemplate`). Under `skip_seen` that key is "seen" after the first
fire, so the second scheduled fire would be dropped forever. Because the cursor already
guarantees fire-timing, **cron-mode instantiation of a def with no discovery treats
dedup as `none`** regardless of the configured value (item dedup is meaningless when
every fire produces the identical item). This makes the slack task fire daily even if
someone later flips its `dedup`. The slack config's explicit `dedup: none` remains
correct and is now belt-and-suspenders.

## Data flow

```
every 2s: daemon interval -> engine.tick() -> pass()
  pass():
    registry.sweep()
    evaluateCrons():                      // NEW
      for def with cron:
        seed cursor if unseen (no fire)
        else if cronDue(cursor, now) and not in-flight:
          cursor = now
          fire-and-forget: instantiateDefinition(def, {mode:"cron"}) -> store.create(...)
    schedule(tasks, live) -> start/resolve/skip     // picks up cron task next tick
```

## Edge cases

- **Boot / hot-reload:** cursor seeds to `now`; nothing fires on start. A slot due
  during a brief restart is missed — tolerated (slack recovers via its 3d watermark
  window; pr-review recovers on the next hourly discovery).
- **macOS sleep:** the process is suspended, not restarted, so the in-memory cursor
  survives. On wake, `(cursor, now]` spans the sleep and `cronDue` fires **once**
  (catch-up-once) — not once per missed slot.
- **Long downtime:** the look-back clamp bounds the `cronDue` minute walk; still at
  most one fire.
- **Slow / hung discovery:** `cronInFlight` guard + immediate cursor advance prevent
  a second concurrent fire; the guard clears when the fire settles.
- **Malformed cron:** `parseCron` throws; the engine logs and skips that def; other
  crons and the rest of the tick are unaffected.
- **Def removed from config:** pruned from the cursor; re-adding re-seeds (no surprise
  catch-up of the gap).
- **Concurrency / lanes:** cron only *enqueues*; the existing lane `schedule()` and
  `maxConcurrentTasks` cap govern when the task actually starts. No change there.

## Testing

Pure `cron.ts` (unit, the bulk of the coverage):
- `parseCron`: `*`, number, list, range, step-on-star, step-on-range; rejects wrong
  field count, out-of-range values, and name tokens.
- `cronMatches`: `0 * * * *` (top of hour), `30 15 * * *` (daily 15:30 local),
  `*/15 * * * *`, `0 9 * * 1-5` (weekdays), dom/dow OR-semantics
  (`0 0 1 * 1` matches the 1st OR any Monday), `7` normalized to Sunday.
- `cronDue`: not due when no minute in window matches; due when the boundary is
  crossed; fires exactly once when the window spans many matching slots (catch-up-once);
  window respects the look-back clamp.

Engine `evaluateCrons()` (unit, with injected fakes for store/exec/clock):
- No fire on first sight (seed only).
- Fires once when a slot comes due; advances cursor.
- In-flight guard blocks a second fire while the first is unsettled.
- Discovery def runs discovery and applies `skip_seen`; discovery-less def enqueues
  exactly one task per due slot even under a stale `skip_seen`.
- Parse error on one def does not stop other defs from firing.
- Deterministic time via an injectable `now()` seam (no wall-clock in tests, matching
  the repo's constraint that `Date.now()` is avoided in pure code).

## Rollout

Development happens on the `cron` branch (this worktree). No config changes needed —
`slack-react-release-notes` (`cron: "30 15 * * *"`) and any re-enabled cron begin
firing as soon as the daemon runs this code. `pr-review`'s cron stays commented out
(disabled per request), so it will not fire until re-enabled.
