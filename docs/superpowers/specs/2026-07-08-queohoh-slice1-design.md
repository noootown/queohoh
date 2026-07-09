# queohoh — Slice 1 Design: Queue-Centric Orchestrator Walking Skeleton

Date: 2026-07-08
Status: approved for planning

## Vision

queohoh is "AI intelligence for a directory": a queue-centric orchestrator for AI
dev work that unifies the tools in daily use today — agent247 (cron agent tasks),
noootown-queue (ad-hoc fix queue), superpowers plan implementation, WorkTrunk
worktrees, tmux layouts — behind one daemon, one task queue, and one terminal
cockpit. It replaces agent247 as a **higher-level orchestrator**, not just a cron
runner; agent247's run-observability layer (tool-call streaming, prettifiers,
cost reporting, config snapshots) is ported forward.

### Core principles

- **Queue as the trunk.** Everything is a task in one central queue: cron-fired,
  human-enqueued, plan implementation, PR review, autotest. The queue is the
  single choke point where scheduling decisions happen — which is what makes
  future policies (priorities, "heavy tasks after midnight", token budgets)
  drop-in rather than re-architecture.
- **Worktree = unit of concurrency.** Tasks targeting the same worktree run
  sequentially (one lane per worktree); different worktrees run in parallel.
- **Files are truth; the daemon acts on them.** All durable state is plain-text
  on disk (cat-able, hand-editable, git-friendly where appropriate). The daemon
  is a watcher/worker-host/API-server that can be killed and restarted freely;
  it rehydrates from files.
- **One daemon per machine**, kept alive by launchd (KeepAlive is launchd's
  *only* job — all scheduling logic lives in the daemon).
- **Terminal-first.** The TUI cockpit is the primary surface (perpetual tmux
  tab 0). Web UI comes later for inherently-web things (litellm/bifrost,
  deep analytics).
- **User's UX collapses to two verbs:** enqueue, and glance at the queue.
  Tasks run end-to-end without babysitting.

### Project model

- **Project = repo**, registered in global config (`projects: [{name, path}]`).
- **Workspace = the user's queohoh home** (`workspace:` key in global config,
  e.g. `~/workspace/queohoh` — git-init it like the agent247 workspace).
  Task definitions and per-project vars live at `<workspace>/<project-name>/`
  — NEVER inside the work repos (personal automation must not be committed
  into employer codebases). *(Amended 2026-07-08: replaces the original
  per-repo `<repo>/.queohoh/` model; queue and runs stay central for now,
  per-project state/runs split deferred.)*
- **Worktree = runtime instance.** Ephemeral per-worktree state (ports,
  sessions) is daemon-tracked, gitignored.

### Slice decomposition (each gets its own spec → plan → build)

1. **Slice 1 (this spec):** daemon core + central task queue + task
   definitions (discovery, dedup, hooks, template vars) + per-worktree
   serialized execution + worktree resolver + minimal Ink TUI + MCP enqueue +
   /qoo skill.
2. Cron engine — time-based triggers on task definitions (the definition/
   discovery machinery itself ships in slice 1) + launchd-parity migration.
3. Environment automation — `wt` + init-tab tmux recipe (nvim / mise / claude
   panes) as one daemon-tracked command; Ctrl-S-style temp workspace spawn.
4. TUI cockpit full build-out.
5. Web dashboard.
6. Model/prompt gateway (bifrost + litellm integration, call observability).
7. Knowledge management — per-directory knowledge config, structured index,
   search (tighten up the current knowledgebase).

## Stack

TypeScript. pnpm workspace monorepo:

```
queohoh/
  packages/
    core/      # task model, queue store, scheduler, resolver, runner — pure logic, no UI deps
    daemon/    # long-running process: file watchers, workers, unix-socket API, MCP server
    tui/       # Ink app — thin client of the daemon API
  docs/superpowers/specs/
```

- **TUI framework: Ink** (React for terminals — what Claude Code itself uses).
  Rationale: mature ecosystem, AI-assistant fluency, direct reuse of agent247
  TS code. Known limitation: high-frequency full-screen updates / huge
  scrollback are Ink's weak spot. **Design guardrail:** the dashboard renders
  ~1s-tick summaries + one drill-in stream at a time; firehose logs open in
  `$PAGER`; high-rate views belong to the web UI (later).
- agent247 code is **ported into `core`** (copied and adapted, not depended
  on): runner/subprocess supervision, redaction, report parsing, prettifiers,
  lock, discovery, dedup, hooks, template variables, MCP tool patterns.
  agent247 retires once queohoh reaches parity.
- Go/Rust were considered and rejected: no high-concurrency need; single-binary
  is a nice-to-have (achievable later via `bun build --compile` if wanted).

## On-disk state

| What | Where | Format | Writer |
|---|---|---|---|
| Global config (registered projects, defaults, `max_concurrent_tasks`) | `~/.config/queohoh/config.yaml` | YAML | user |
| **Task queue** | `~/.local/state/queohoh/tasks/<ulid>.md` | md + YAML frontmatter | user + daemon |
| Archived tasks | `~/.local/state/queohoh/archive/` | same | daemon |
| Run logs / reports / snapshots | `~/.local/state/queohoh/runs/<task-id>/` | jsonl / md | daemon |
| Daemon runtime (pid, socket, session registry) | `~/.local/state/queohoh/daemon/` | JSON | daemon |
| Task definitions | `<workspace>/<project>/tasks/<name>/` | YAML+md, user's workspace repo | user |
| Per-project vars | `<workspace>/<project>/vars.yaml` | YAML, optional | user |
| Per-worktree runtime | `<worktree>/.queohoh/runtime.json` | JSON, gitignored | daemon |

Conventions: YAML for hand-written config, Markdown for hand-touchable queue
items, JSON for daemon-owned machine state. All daemon writes to task files are
atomic (write-temp + rename) so hand-edits and daemon writes cannot torn-write.
The daemon watches `tasks/` — dropping a well-formed file there IS an enqueue.

### Task file format

```markdown
---
id: 01J9XK...        # ulid — sortable by creation time
status: queued        # queued | needs-input | running | done | failed
definition: platform/pr-review   # or `adhoc`
item: { number: 1423 }           # frozen discovered/arg item (absent for adhoc)
item_key: "1423"                 # dedup identity (absent for adhoc)
target:
  repo: platform
  ref: "pr:1423"      # pr:N | ticket:JUS-N | worktree:name | temp
  worktree: null      # filled by resolver at schedule time
priority: normal      # low | normal | high
created: 2026-07-08T10:12:00Z
source: mcp           # mcp | tui | cron (slice 2)
---
Reply to Kevin's review comments on PR #1423, then re-run the affected tests.

## Attachments
(transcribed context — same sidecar convention as noootown-queue)
```

## Task model: Definitions and Instances

Two-level model (inherited from agent247, adapted to the queue):

**Task Definition** — *the recipe.* Lives in the user's workspace, grouped by
project: `<workspace>/<project>/tasks/<name>/config.yaml` + `prompt.md`
(agent247's task-folder format, nearly verbatim; extra scripts like
`discover.sh` sit alongside — discovery/hook commands run with
cwd = `<workspace>/<project>/`, matching agent247's workspace-cwd semantics):

```yaml
# <workspace>/platform/tasks/pr-review/config.yaml
discovery:
  command: gh pr list --search "review-requested:@me" --json number,title,headRefName
  item_key: "{{number}}"
args: [number]            # optional named params for manual arg invocation
dedup: skip_seen          # skip_seen | retry_errored | none
worktree: "pr:{{number}}"   # ref template → feeds the resolver
pre_run: "mise run setup"        # optional, runs in resolved worktree
post_run: "..."                  # optional, finally-semantics
model: opus
timeout: 30m
priority: normal
```

**Task Instance** — *a unit of queued work.* What the central queue holds
(file format below), carrying `definition: <repo>/<name>` (or `adhoc`), the
frozen discovered `item` JSON, and its `item_key`.

**Triggers on a definition:**

1. **Cron** (slice 2) — time-based trigger, same pipeline.
2. **Manual run** — "run now" from TUI/MCP; discovery finds the items.
3. **Manual with args** — e.g. `pr-review 257`: args map onto the declared
   `args` fields to form the item directly; discovery is skipped, dedup still
   applies.

**Pipeline:**

```
trigger → discovery.command → JSON items
        → dedup (item_key vs existing instances + archive, per policy)
        → instantiate: one queue instance per surviving item
          (worktree ref rendered from template)
        → normal queue flow: resolve → pre_run → claude run
          (prompt rendered from global vars + repo vars + item fields,
           snapshotted) → post_run (always) → report
```

- **Adhoc is a definition-less instance**: the typed text is the prompt; no
  discovery/dedup; hooks empty. One execution path for everything.
- **Resolver vs hooks:** the resolver owns standard worktree find/create;
  `pre_run`/`post_run` are the escape hatch for extra setup/teardown and run
  *inside* the already-resolved worktree (unlike agent247, hooks never
  `wt switch` themselves).
- **Template variables**: global vars (global config, agent247 `vars.yaml`
  equivalent) + per-repo vars + item fields, substituted into prompt,
  discovery command, hooks, and worktree ref.

## Task lifecycle

```
            resolver can't match          user answers via TUI
  queued ──────────────────────► needs-input ──────────► queued
    │ scheduler picks it (lane free + global slot free)
    ▼
  running ──► done ──► archive (configurable, default 7 days)
    │
    ▼
  failed ──► TUI: retry → queued | skip → archive
```

- `needs-input` parks a task without blocking any worker.
- **Lane pause on failure:** a `failed` task holds its worktree's lane until
  the user retries or skips — a failed task usually means a dirty/broken tree
  the next task would trip over (noootown-queue's halt rule, carried forward).
- Instances come from task definitions (see Task model above) or are
  `adhoc` (free-text prompt, run end-to-end, commit).

## Scheduler

A pure function over (task list, live lane state), re-evaluated on any change:

- One lane per worktree; at most one running task per lane; FIFO within
  priority band (`high` → `normal` → `low`).
- Global `max_concurrent_tasks` cap (default ~3) across all lanes — the
  machine/token throttle, and the seam where future policies plug in
  (a policy = a function that filters/orders eligible tasks).
- Lane pause on failure (above).
- **Interactive-awareness:** if a human Claude session is live in a worktree,
  that lane is treated as busy — interactive work always outranks queued work.

## Worktree resolver

Deterministic chain, no LLM inference, evaluated **at schedule time** (not
enqueue time, so late-appearing worktrees are picked up):

1. `worktree:<name>` → use if exists, else `needs-input`.
2. `pr:<N>` → `gh pr view` → branch name → existing worktree on that branch?
   use it. Else extract Linear ticket id from branch name (convention:
   branch/worktree named by ticket, e.g. `JUS-1423`) → worktree exists? use
   it : **spawn** it via `wt`.
3. `ticket:<JUS-N>` → worktree named `<JUS-N>` → exists? use : spawn.
4. `temp` → spawn `tmp-<slug>-<shortid>`, marked **ephemeral**: on task
   archive, auto-delete if tree is clean and pushed, else leave and flag in
   the TUI.
5. No confident match → `needs-input`; TUI shows a picker (existing worktrees
   + "new…"). Never guess.

Spawning shells out to `wt` (WorkTrunk). Headless workers need no tmux panes;
the full init-tab layout recipe is slice 3.

## Worker & run observability (the agent247 inheritance)

A worker executes one task end-to-end in its lane's worktree:

- Spawns headless `claude -p` with the task prompt. Ported from agent247:
  process supervision, per-worktree lock, timeout (per-type, default ~30m →
  SIGTERM → `failed`), cancellation, env handling, redaction.
- Streams `stream-json` events to `runs/<task-id>/events.jsonl`; agent247's
  **prettifiers** render live tool-call-by-tool-call views for the TUI
  drill-in.
- **Run report** per task (ported `report.ts`): result summary, turns,
  duration, model + cost breakdown, files touched, commits made.
- **Config snapshot** per run: exact prompt, model, per-repo config, and
  resolver decision frozen into `runs/<task-id>/snapshot/`.
- Completion contract (`adhoc`): work → verify → commit. `done` only on clean
  exit + clean-or-committed tree; anything else → `failed` with the report
  explaining why.

## Session registry

- Workers register `{worktree, task, pid, started}` in daemon runtime state.
- Human sessions: a lightweight Claude Code hook (SessionStart/Stop) pings the
  daemon — "interactive session live in `<cwd>`". Best-effort: entries expire
  by heartbeat age; no hard dependency on the hook.
- Feeds scheduler interactive-awareness and the TUI worktrees panel
  ("who is writing where").

## TUI dashboard (cockpit)

Ink app; connects to the daemon unix socket and **subscribes** to pushed state
changes (never polls files). Meant to sit open perpetually in tmux tab 0.

Layout: **two columns — wide queue column left** (it also hosts detail
drill-in: report, transcript, prompt, run log, data), **narrow right column
stacking cron (slice 2 slot) above worktrees**:

```
┌ QUEUE ──────────────────────────────┬ CRON (slice 2) ────┐
│ running: spinner + current tool     │ defs, next fire,   │
│   call + elapsed + running cost     │ last result        │
│ queued: lane position               ├ WORKTREES ─────────┤
│ needs-input: ？ + picker on enter    │ lane state, dirty  │
│ recent done/failed inline (~10)     │ flags, ephemeral   │
│                                     │ pending teardown   │
└ [a]dd [enter]detail [e]dit [d]el [J/K]reorder [p]rio [r]etry [s]kip ┘
```

- Queue management happens here: quick-add (adhoc), run a task definition
  (pick from list, optionally input args — e.g. `pr-review 257`), edit,
  delete, reorder within lane, priority bump, retry/skip, answer needs-input.
- Drill-in (`enter`): full-screen live prettified tool-call stream, then the
  run report. One stream at a time; big logs → `$PAGER`.
- A TODAY strip aggregates run counts + cost from reports.
- Daemon down → banner + auto-reconnect; TUI is stateless.
- **UI details are expected to be tweaked over time** — this spec pins the
  information architecture (what is visible, what actions exist), not exact
  layout/pixels.

## MCP enqueue tool

The daemon exposes an MCP server (stdio bridge, agent247 `mcp.ts` pattern):

- `enqueue_task(prompt, repo, ref?, priority?)` — adhoc instance.
- `list_task_definitions()` — all definitions across registered repos.
- `run_task_definition(repo, name, args?)` — trigger a definition (with or
  without args).
- `list_tasks()` — queue state.

`repo` is required on both enqueue tools because task definitions are per-repo
namespaced and the worktree resolver needs the project to locate/spawn the
target worktree.
- Registered once in global Claude Code config → every session can enqueue
  mid-conversation (successor to /noootown-queue).
- Attachment convention ported: the calling session transcribes images/rich
  context into the task body before enqueueing (workers never see pastes).

### /qoo skill

A thin Claude Code skill shipped with slice 1: the user describes what they
want ("review Kevin's auth PR"); the *interactive session* calls
`list_task_definitions()`, picks the matching definition, extracts args, and
calls `run_task_definition` (or falls back to `enqueue_task` adhoc). Fuzzy
natural-language matching happens in the session the user is watching and can
correct — the daemon core stays fully deterministic.

Enqueue surfaces in slice 1: **TUI quick-add + MCP tool.** (A `qo add` CLI is
trivial later — the daemon API is the real interface; cron calls it in
slice 2.)

## Recovery & failure model

- Daemon restart → rescan `tasks/`, rebuild lanes. `running` tasks whose
  worker pid is dead → `failed` ("orphaned by daemon restart"); lane-pause
  makes the user look at the worktree before anything else touches it.
- Single-daemon lock: pidfile + socket probe (agent247 `lock.ts` pattern).
- launchd KeepAlive supervises the daemon process; nothing else.

## Testing

- `core` unit-tested hard (pure logic): scheduler (lane rules, priority,
  pause, interactive-block), resolver (each chain step with stubbed
  `gh`/`wt`), task-file parse/serialize round-trip.
- `daemon` integration tests: tmp state dir + fake `claude` script emitting
  canned stream-json (agent247 test patterns ported).
- `tui`: ink-testing-library render tests; stream renderer tested against
  recorded event fixtures.

## Out of scope for slice 1

Time-based cron triggers (2), tmux/env recipes and Ctrl-S spawn UX (3), full
cockpit build-out (4), web UI (5), model gateway (6), knowledge management
(7), scheduling policies beyond FIFO+priority (midnight batching, token
budgets), `qo` CLI, multi-machine anything. agent247's network check
(`requires_network`) is not ported unless a real need appears.

## Migration notes

- noootown-queue: retire after slice 1 — its `.agents/todo.md` habit is
  replaced by the MCP enqueue tool; sidecar/attachment convention carries over.
- agent247: keeps running crons until slice 2 reaches parity, then retires.
