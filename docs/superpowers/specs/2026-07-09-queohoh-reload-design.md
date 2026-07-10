# `queohoh reload` — Rebuild + Restart the Daemon

**Date:** 2026-07-09
**Status:** Approved design

## Problem

The daemon is a long-running Node process: it serves whatever
`packages/daemon/dist/` looked like when it started. After changing daemon
code the operator must remember to rebuild AND restart, and a careless
restart mid-run orphans in-flight tasks (the next daemon's orphan sweep
marks them `failed: "orphaned by daemon restart"`). There is no single
command that does this safely.

## Goal

One command — `queohoh reload` — that rebuilds the checkout the CLI lives
in and restarts the daemon on the fresh build, refusing (by default) to
kill in-flight work.

## Behavior

`queohoh reload [--force]`, implemented as a new commander command in
`packages/daemon/src/cli.ts` with the logic in a new
`packages/daemon/src/reload.ts` (exec, ports, and paths injectable for
unit testing).

1. **Locate the repo root** from the CLI's own location
   (`fileURLToPath(import.meta.url)` = `<root>/packages/daemon/dist/cli.js`
   → four segments up, since `cli.js` itself counts as one). Sanity-check
   that `pnpm-workspace.yaml` exists
   at the root; otherwise exit 1 with a clear message (binary not inside a
   checkout).
2. **Busy guard.** Call `state` on the daemon socket. If `running` is
   non-empty, print the running task ids and exit 1 without touching
   anything. `--force` skips the guard and states in its output that the
   running tasks will be marked failed by the orphan sweep. If the daemon
   is unreachable, nothing is running — the guard passes.
3. **Build before killing.** Run `pnpm -r build` in the repo root with
   streamed output. On nonzero exit, abort — the old daemon keeps running.
   A failed build must never trade a working daemon for a broken one.
   No `git pull`, no `pnpm install`: reload builds exactly what is checked
   out; dependency changes surface as build failures.
4. **Restart.**
   - launchd-managed (detected by `launchctl print
     gui/<uid>/com.queohoh.daemon` exiting 0) → `launchctl kickstart -k
     gui/<uid>/com.queohoh.daemon`; KeepAlive relaunches on fresh dist.
     The launchd branch is only taken when `QUEOHOH_STATE_DIR` is unset:
     a launchd-managed daemon always runs on the default state dir, so an
     overridden state dir (hermetic tests, alternate deployments) must
     use the pidfile path rather than kick a daemon it isn't talking to.
   - Otherwise → read the pidfile, SIGTERM, wait up to 5s, SIGKILL if
     needed, then spawn a detached `node <cli> daemon` appending to the
     existing `daemon/daemon.log`.
   - No daemon running at all → just start one (reload doubles as
     "ensure a fresh daemon").
5. **Verify.** Poll the socket until reachable (up to ~5s). On success
   print a confirmation; on failure exit 1 and print the last lines of
   `daemon.log`.

## Testing

Unit tests for the injectable decision logic in `reload.ts`: repo-root
derivation and sanity check, busy-guard outcomes (idle / busy / busy with
force / daemon unreachable), build-failure abort ordering (kill never
attempted), launchd-vs-pidfile branch selection. The actual
launchd/kill/spawn side effects are exercised manually — they are
process-level and mirror the proven `scripts/daemon-ensure.sh` sequence.

## Out of scope

- `git pull` / `pnpm install` automation.
- Waiting for the queue to drain (`--wait`); refuse-fast is the contract.
- Changes to `scripts/daemon-ensure.sh` (kept as the script-level
  self-heal; `reload` is the human-facing path).
