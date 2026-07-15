# Detached run shim — daemon-agnostic `claude -p` runs

## Problem

The daemon runs every task in-process: `executeClaude` (core/runner.ts) spawns `claude -p` detached, but the daemon holds the stdout pipe, owns the idle/ceiling reapers in memory, and does all post-run bookkeeping (outcome classification, verify gate, post_run, lineage, chain advance) inline. A daemon death — reload or crash — breaks the pipe and loses the run: the orphan sweep marks on-disk `running` tasks `failed: orphaned by daemon restart`, and `reload.ts` carries a busy guard that refuses to rebuild while anything is running unless `--force`.

Goal: **full detach + re-adopt.** Running claude sessions survive any daemon death (graceful reload or crash). A returning daemon re-adopts them: picks up their result, runs verify/post_run, advances chains. Reload no longer needs `--force` or an idle queue.

## Design overview

A per-run **shim process** becomes claude's parent instead of the daemon. The daemon's job shrinks to: spawn the shim, then watch the run directory. All run-lifecycle state that must survive the daemon moves into the shim or onto disk; all task-state writes stay in the daemon (single-writer invariant holds).

```
daemon ──spawn(detached, unref)──▶ node dist/shim.js <runDir>
                                     1. read spawn.json (0600), unlink it
                                     2. redactor = makeRedactor(buildSecretMap(process.env))
                                     3. result = await executeClaude(opts)   ← existing core fn, unchanged
                                     4. atomically write result.json (tmp+rename)

daemon (any instance, incl. post-restart):
  run dir → result.json appears → finalizeRun: classify / verify /
  post_run / lineage / chain advance / task-store update
```

## Components

### The shim (`packages/daemon/src/shim.ts` → `dist/shim.js`)

Deliberately thin — a CLI wrapper around the existing `executeClaude`, which moves with all its machinery intact: stream-json parsing, events/transcript writing, idle reaper (12 min) + ceiling (per-task timeout), SIGTERM→SIGKILL group-kill logic.

- Reads `spawn.json` from the run dir, unlinks it immediately after parse (it holds the unredacted prompt; written mode 0600).
- Builds its redactor from its own environment — `makeRedactor(buildSecretMap(process.env))` — which is identical to the daemon's (inherited env), so no secrets cross the file boundary.
- Claude stays `detached: true` under the shim (own pgroup, so grandchild bash processes die with it, exactly as today).
- Traps SIGTERM and forwards it to claude's process group; `executeClaude`'s close handler records the signal into the result and the shim exits.
- Writes `result.json` via tmp + rename — the daemon can never read a torn file.
- Rebuild-safe: node loads the module graph at startup, so overwriting `dist/` mid-run does not affect a live shim.

### Run-dir file contract

| File | Writer | Content |
|---|---|---|
| `spawn.json` | daemon, before spawning shim | rendered prompt, model, cwd, timeoutMs, resumeSessionId — the `ExecuteClaudeOptions` inputs. Mode 0600; shim unlinks after parse. |
| `worker.json` | daemon writes shim pid; shim adds claude's pid | today it stores the daemon's own pid (useless post-restart); now it is the actual supervisor pid |
| `result.json` | shim, on claude exit | full `RunResult`: exitCode, signal, timedOut, sessionId, resultText, stderr, usage. Atomic. |

Events/transcript files: same paths, same formats, still written incrementally (now by the shim). The TUI reads them directly from disk as before — zero TUI change.

### What deliberately does NOT move into the shim

Outcome classification (session-limit / out-of-budget regexes), verify gate, pre/post_run hooks, lineage recording, task-store writes. The shim writes run-dir files only; the daemon stays the single writer of task state.

## Daemon-side changes

### worker.ts splits at the spawn boundary

- **`startRun`** — status→running, worktree/def/model resolution, snapshot write, pre_run hook, write `spawn.json`, spawn shim, persist shim pid. Existing logic, stops at the spawn.
- **`finalizeRun(result)`** — outcome classification ladder (verbatim), verify gate, post_run, lineage fork recording, `finishRun`, task-store update. Rebuilds context (def via `loadDef`, worktree context via git) from the persisted task — nothing requires memory carried over from `startRun`.

Live path: `startRun` → await result → `finalizeRun`. Adopted path: `finalizeRun` fired by the sweep. One settling code path regardless of which daemon instance started the run.

### Result delivery

While alive, the daemon is the shim's parent: it awaits the child `close` event, then reads `result.json`. For adopted runs there is no child handle; the engine's existing tick polls the run dir. No new timers or fs.watch machinery.

### Orphan sweep → adoption sweep (engine.ts `pass()`)

For each on-disk `running` task not in `this.running`:

1. `result.json` exists → finished while we were away: `finalizeRun` now.
2. Shim pid (from `worker.json`) alive → **adopt**: re-register in `this.running`, `childPids`, and the session registry (lane occupancy stays correct; scheduler won't double-book the lane); keep polling. Pid-reuse safeguard: one-time `ps` check that the pid's argv contains `shim.js`; a recycled pid falls through to case 3.
3. Neither → genuinely orphaned (shim SIGKILLed / never spawned): `failed: worker died`. Also gracefully handles pre-upgrade leftovers, whose `worker.json` holds the old daemon's dead pid.

### Stop

`stopTask` reads the shim pid (in-memory `childPids`, repopulated by adoption — Stop now works on adopted runs, fixing the current "started under a previous daemon process" throw). It writes a `cancelled` marker file into the run dir **before** signalling, then SIGTERMs the shim. `finalizeRun` treats marker-or-in-memory-flag as user stop → `cancelled`. The marker closes today's in-memory-only gap: stop followed by daemon death still settles as `cancelled`, not `failed`.

### Reload

The busy guard (`busyVerdict` and its call site) is deleted — reload becomes build → restart → verify, unconditionally. `--force` stays accepted as a no-op so muscle memory and scripts don't break. `scripts/daemon-ensure.sh` untouched.

### Wire/TUI compatibility

Zero changes. Status values, snapshot shape, run-file formats identical. Only user-visible string change: `orphaned by daemon restart` no longer occurs (replaced by the rarer, accurate `worker died`).

## Error handling

| Failure | Behavior |
|---|---|
| `spawn.json` write fails | Fail the task before any spawn (mirrors today's events-file init guard) |
| Shim can't spawn claude | `executeClaude` resolves `exitCode 1 / "Failed to spawn process"`; shim writes it as `result.json`; daemon classifies failed. Never a silent hang. |
| Shim crashes mid-run | No `result.json`, pid dead → sweep case 3 → `failed: worker died`. Residual risk: a claude child orphaned by a non-signal shim crash keeps running unsupervised — same exposure as today's `--force` path; accepted. |
| Daemon dies between status→running and shim spawn | No pid, no result → case 3, failed. Correct. |
| Wedged claude while daemon is down | Shim's own idle reaper / ceiling fire regardless of daemon liveness — strictly better than today. |
| Torn `result.json` | Impossible (tmp + rename). Parse failure treated as absent → keep polling; pid dead + unparseable → case 3. |

## Testing

- **Unit (core/worker):** existing worker tests carry over — classification/verify/hook logic lands in `finalizeRun` with injectable deps as today.
- **Unit (daemon):** adoption decision as a pure function `(hasResult, pidAlive, argvLooksLikeShim) → finalize | adopt | orphan`, table-tested. `busyVerdict` tests deleted with the guard.
- **Integration:** one shim round-trip — spawn real `dist/shim.js` with a fake claude script (pattern exists in runner tests), assert `result.json` contents and that events/transcript match today's formats byte-for-byte.
- **Manual acceptance:** start a long run, `qoo reload` mid-flight, confirm the transcript keeps growing, the daemon comes back, adopts, and the task settles `done` with verify/post_run having run.

## Observable behavior changes (all improvements)

- `qoo reload` works any time; no `--force`, no waiting for idle.
- Tasks survive daemon restarts and crashes; completions during downtime are finalized on adoption (verify/post_run a few seconds later than they would have been — the only timing difference).
- Wedged runs are reaped even while the daemon is down.
- Stop works on runs started by a previous daemon instance.
