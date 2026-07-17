# @queohoh/daemon — agent guide

The process host. It owns the long-running daemon: the tick loop that drives scheduling, the unix-socket JSON-RPC API the TUI subscribes to, the MCP server Claude sessions call, per-worktree git/PR enrichment, worktree lifecycle, and the single-instance lock. It wires `@queohoh/core`'s seams to a real process — all domain logic lives in core; this package is orchestration + IO.

## Hierarchy — what owns what

```
src/
  daemon.ts       Composition root: builds Engine + ApiServer, wires the tick
                  loop (see below), takes the lock, owns shutdown.
  engine.ts       Engine: the tick pass (schedule → apply skips → resolve →
                  start), worker lifecycle, worktree create/remove, git/PR
                  enrichment, stopTask, chain worktree stamping. Largest file.
  api.ts          ApiServer: unix-socket newline-JSON-RPC, StateSnapshot shape,
                  dispatch() = the RPC surface (the TUI's contract).
  mcp.ts          MCP server: tool registration + zod schemas (the Claude-facing
                  contract). Tools are thin — they call RPCs.
  mcp-tools.ts    mcp* wrappers: shape args → one RPC call → ToolResult. No
                  business logic; never touch the store directly.
  client.ts       ApiClient: socket client used by cli, reload, mcp, tests.
  cli.ts          commander CLI: daemon | status | reload | launchd:install |
                  launchd:uninstall | systemd:install | systemd:uninstall |
                  mcp | heartbeat.
  reload.ts       `reload`: rebuild + restart (launchd / systemd --user when
                  active, else pidfile + detached re-spawn).
  launchd.ts      launchd plist renderer (macOS keep-alive).
  systemd.ts      systemd user unit renderer (Linux keep-alive; KillMode=process
                  so detached run shims survive restarts).
  lock.ts         acquireLock: single-instance pidfile lock (atomic, stale
                  takeover). See decision below.
  build-id.ts     currentBuildId(): build fingerprint for TUI self-heal.
  paths.ts        Filesystem locations (state dir, config path, socket path).
  index.ts        Barrel.
```

## Decisions (the why)

- **Tick model.** `daemon.ts` drives `engine.tick()` from three sources: a 2s `setInterval` (unref'd, the steady heartbeat), an `fs.watch` on the state `tasks/` dir (external enqueues — a dropped task file IS an enqueue), and `onMutation`/`onChange` kicks after an RPC that changed state. Every tick ends with `server.broadcast()`. `tick()` is single-flighted (re-entrant calls no-op), so overlapping kicks are safe.
- **Enrichment stays off the hot path.** `refreshGitEnrichment` is fire-and-forget from `pass()`, single-flighted, and TTL-throttled at 60s per worktree. It must never add latency to scheduling. One `git log` call per worktree with a single positional format string (`%ct%x09%an%x09%ae%x09%h` → epoch/author/email/hash) — parsed positionally so an older/shorter line degrades to nulls, not a crash. PR numbers: at most ONE `gh pr list` per repo per sweep, fetched lazily (only when a worktree is actually refreshing), failure-tolerant (gh missing/unauth/bad-JSON → null everywhere, never throws).
- **Snapshot contract is additive-only.** `StateSnapshot` is camelCase JSON over the socket. Old TUIs MUST keep working: only ADD optional fields, never rename or remove. The Rust side (`crates/qoo-tui/src/ipc/types.rs`) tolerates unknown status strings via serde `#[serde(other)] → Unknown` and unknown fields via container `default` — that tolerance is what makes a new TaskStatus or field safe, but a rename still breaks it. Mirror any new snapshot field there.
- **RPC surface (api.ts) vs tool surface (mcp.ts) are separate contracts.** The TUI speaks RPCs; Claude speaks MCP tools. MCP tools are thin wrappers that call RPCs — add business logic to the RPC dispatch, not the tool. The `skip` RPC is dual-role: it CANCELS a live task (queued/needs-input → `cancelled`, stays visible) but ARCHIVES an already-terminal one (dismiss).
- **stop → cancelled, not failed.** `stopTask` records the id in `cancelledTaskIds` before killing (SIGTERM group, 5s SIGKILL escalation); the worker reads `isCancelled` and settles the resulting signal as `cancelled` ("stopped by user"). A signal WITHOUT that flag (external/OOM kill) stays `failed` — a real crash isn't masked.
- **Chain worktree stamping.** When a chain HEAD resolves, the engine stamps the resolved worktree onto every other member (pins `ref: worktree:<name>`, clears ephemeral) so tails share the lane and never re-resolve. Scheduler skips are applied here (status → `skipped`) before resolve/start.
- **Auto-archive.** Old `done` AND `cancelled` tasks are auto-archived after `archive_after_days` (cancelled is a deliberate, resolved outcome). `failed` and `skipped` are left visible — they usually want attention or explain a stalled chain.
- **Single-instance lock.** Atomic exclusive create (`wx`) closes the TOCTOU window; on an existing pidfile, a live owner wins and a stale one is taken over with a re-read confirm. Two daemons can never run together.
- **dist/ is what runs.** The daemon executes compiled `dist/`, not the TS source (vitest aliases core to source, but the process does not). `build-id.ts` fingerprints the build so the TUI can detect a stale daemon and self-heal.

## Conventions (do this)

- **Additive-only snapshot changes**, and mirror each new field into `crates/qoo-tui/src/ipc/types.rs` (read-only from here — coordinate; do not edit crates). Never rename/remove a snapshot field.
- **New TUI capability → new RPC** (a `dispatch()` case in api.ts). **New Claude capability → new MCP tool** (mcp.ts) + a thin `mcp-tools.ts` wrapper that calls the RPC. Keep the two surfaces in sync only where they must be.
- **Keep enrichment (and any new per-worktree/per-repo shelling) off `pass()`**: fire-and-forget, TTL-throttled, single-flighted. Scheduling latency is sacred.
- **After changing daemon code, rebuild + reload** (`pnpm -r build` then `queohoh reload`) — the running process is `dist/`, so source edits are inert until then. `reload` refuses while tasks run (`--force` overrides).
- **Renaming/removing an MCP tool or RPC method is a breaking change** for the TUI and any live Claude session — treat it like the snapshot contract.
