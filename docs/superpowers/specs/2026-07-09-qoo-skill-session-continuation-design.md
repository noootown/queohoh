# /qoo — Universal Queue Skill with Session Continuation

**Date:** 2026-07-09
**Status:** Approved design

## Problem

After a long interactive discussion with Claude, the follow-up work (an
implementation, a PR-ready pass, a doc rewrite) can take hours. Today the
only options are to let it run inline — occupying the tmux tab — or to
re-explain everything to a fresh queued agent. Neither is acceptable: the
interactive session already holds all the context.

The existing `skills/qoo` skill routes requests to task definitions and
ad-hoc fresh-agent tasks, but has no way to queue a run that **continues
the current session**. The daemon's `session: "main"` mode only chains
worker-created sessions; nothing ever connects an interactive session's ID
to the queue, and the MCP `enqueue_task` tool exposes neither `session`
nor `worktree`.

## Goal

`/qoo` is the single skill interface to the entire queohoh framework, used
globally from any project. Its core new capability: queue a headless run
that resumes the current interactive session in the current worktree, so
the user can close the tab and let the daemon do the hours-long work.

## UX

Installed globally via the existing symlink (`ln -s .../skills/qoo
~/.claude/skills/qoo`); the repo remains the source of truth.

- `/qoo status` → `list_tasks`, compact table (kept from current skill).
- `/qoo <request>` → enqueue flow:

1. **Resolve context.** Current worktree = git toplevel of cwd. Current
   session ID = UUID segment of the scratchpad directory path (fallback:
   newest `*.jsonl` under `~/.claude/projects/<munged-cwd>/`). Current
   model = the session's own model.
2. **Match against definitions.** `list_task_definitions`, match on
   meaning. Exactly one fit → use it, asking one short question only if
   its args/mode are genuinely ambiguous (e.g. `/qoo pr-ready` → ask which
   mode). Multiple fits → ask the user to pick. No fit → ad-hoc enqueue.
   Never invent definition names.
3. **Session mode: continuation, always.** Every queued run resumes the
   current session in the current worktree unless the user explicitly says
   "fresh". Applies to definition runs too.
4. **Handoff prompt (ad-hoc only; definitions bring their own).**
   Task-shape-agnostic template: restate the concrete deliverable and its
   done-condition as agreed in the conversation, fold in the `/qoo`
   arguments verbatim, add verification steps if the task has a runnable
   check, and always end with "commit all work when done" (the worker
   marks dirty-tree runs failed).
5. **Dirty-tree check.** If the worktree has uncommitted changes at queue
   time, tell the user and include a prompt line instructing the run to
   handle/commit them.
6. **Report, one line:** what was queued, target `repo:worktree`, which
   session it resumes, which model, plus: "queued on `repo:worktree`; watch
   it in the TUI." Note: interactive/main sessions no longer hold their
   lane, so a continuation run is not deferred until the session goes idle —
   it starts as soon as the scheduler has a free slot and no worker is
   already running on that lane. Avoiding a fork of a session the user is
   still typing into is now the caller's responsibility (queue the
   continuation when you're done in the tab), not a scheduler guard.

## Architecture

### Resume wiring: pin + chain-advance

A task carries an explicit `resume_session_id` (the pin). When several
follow-ups are queued from one session, each should resume the *previous
run's resulting session*, not the original snapshot — lanes serialize, so
the chain is well-defined.

Worker resolution at spawn time:

- `task.resumeSessionId` set → use the lane's `MainSessionStore` pointer
  **only if** `pointer.updatedAt > task.created` (a descendant run
  finished after this task was queued); otherwise use the pin. A stale
  pointer from an old chain can never hijack a new pin.
- Else `session === "main"` → pointer, as today.

After any run that captured a `sessionId`, advance the lane pointer when
the task was `main` **or** pinned.

### Core changes

- **`core/task.ts`** — two new optional frontmatter fields, `null` by
  default: `resume_session_id` and `model` (per-task override; without it
  a fable session's continuation silently downgrades to the hardcoded
  `sonnet` default in `engine.ts`). Existing task files parse unchanged.
- **`core/main-sessions.ts`** — entries become
  `{ sessionId, updatedAt }`; loader accepts the legacy bare-string form
  and upgrades it.
- **`core/worker.ts`** — resume resolution as above; model precedence
  `def.model ?? task.model ?? defaults.model`; pointer advance rule.
- **`core/instantiate.ts`** — accepts `resumeSessionId`, stamped on every
  task the definition instantiation creates.

### Daemon changes

- **`daemon/api.ts`** — `enqueue` gains `cwd`, `resume_session_id`,
  `model`. Given `cwd`, the daemon resolves `{repo, worktree}` by matching
  against configured project paths and their git worktrees (including the
  primary checkout). Unresolvable → error carrying the exact `projects:`
  snippet to add to `~/.config/queohoh/config.yaml` (fail with guidance —
  no auto-registration). `runDefinition` gains `cwd` (feeding the existing
  `refOverride`) and `resume_session_id`.
- **`daemon/mcp.ts` / `mcp-tools.ts`** — `enqueue_task` and
  `run_task_definition` schemas extended with the new params. No new
  tools.

### Skill rewrite

`skills/qoo/SKILL.md` rewritten to implement the UX flow above. Routing
and rules from the current skill that survive: `status` mode, definition
matching on meaning, "never do the work inline", "never invent definition
names", prefer sensible defaults with minimal clarifying questions.

## Edge cases

- Daemon down / MCP unavailable → skill reports it and points at
  `queohoh daemon` / `queohoh launchd:install`.
- Session-ID discovery fails both ways → fail with a message; never queue
  a "continuation" that would silently run fresh.
- User keeps working in the session after queueing → heartbeat keeps the
  lane blocked (by design); the report line warns that continued typing
  diverges from the fork point.
- Repo not registered in queohoh config → error with the exact config
  snippet to add.
- **Verification item:** confirm `claude -p --resume <id>` resolves a
  session by ID when invoked from the worktree root even if the
  interactive session was launched in a subdirectory. If it fails, key on
  transcript-dir munging instead.

## Testing

- Task parse/serialize round-trip for the new fields (absent, null, set).
- Worker resume precedence with a fake executor: pin wins with no
  pointer; fresh pointer (updated after task creation) wins; stale
  pointer ignored; `main` mode unchanged; pointer advance after pinned
  runs (done and failed outcomes).
- MainSessionStore legacy bare-string migration.
- `cwd` → lane resolution, including primary checkout and the
  unregistered-repo error.
- MCP tool param threading, matching existing `mcp-tools` test patterns.
- Manual e2e: queue a continuation from a real session, close it, watch
  the daemon resume with context intact.

## Out of scope

- Auto-registration of unknown repos.
- Standardized `plan-implement`-style task definitions with model args
  (future work; the skill's definition-arg clarification flow already
  accommodates them).
- TUI changes (run meta already records the resulting session ID).
