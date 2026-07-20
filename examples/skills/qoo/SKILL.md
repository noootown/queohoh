---
name: qoo
description: Queue work onto the queohoh orchestrator instead of doing it inline. Turns the user's request into a headless queued run (or an ordered chain for multi-step requests) via the queohoh MCP server. Requires the queohoh daemon and MCP server (`claude mcp add queohoh -- queohoh mcp`).
user-invocable: true
argument-hint: "<what you want done> | status"
---

# /qoo — queue it instead of doing it

Turn the request into a queued queohoh run. Your job is ONLY to route and enqueue — never do the work yourself; the point of the queue is that the user stays in flow.

**Input:** `$ARGUMENTS` — free text describing the work, or exactly `status`.

## Routing

- `status` → call `list_tasks` and render a compact table (id, status, repo:worktree, first ~60 chars of prompt). Done.
- Anything else → enqueue, below.

## Enqueue

1. **Try a definition first.** Call `list_task_definitions` and match the request by meaning, not string equality. Exactly one fits → `run_task_definition(repo, name, args?)`. Never invent definition names.
2. **Multi-step request** ("A, then B") → `enqueue_chain(steps=[...])`. Each step is `{definition, args?}` or `{prompt}`; steps run FIFO in one shared worktree, and a failed step skips the rest.
3. **Otherwise** → `enqueue_task(prompt=...)`. The worker sees ONLY the prompt (unless you pass `resume_session_id`), so make it self-contained: transcribe error messages, file paths, and anything discussed. Always end with "Commit all work when done — do not leave the tree dirty."

Targeting, for any of the three:

- `cwd=<absolute path in a worktree>` → run right there.
- `ref: pr:<N> | ticket:<ID> | worktree:<name> | temp` → the daemon reuses or spawns a matching worktree (`temp` = fresh throwaway).
- `verify="<shell command>"` → done-condition the framework runs after the worker claims success; non-zero lands the task `verify-failed`. Set it whenever the task has a runnable check (tests, build, lint).

## Report

One line: what was queued, the target repo:worktree (or ref), and that progress is visible in the TUI (`mise run tui`). The run starts within seconds — enqueue as your last action.

## Failure modes

- MCP tools missing or connection refused → the daemon isn't running: `queohoh daemon` (foreground) or `queohoh launchd:install`.
- "no registered project contains ..." → relay the error verbatim; the fix is in the user's `config.yaml`, not here.

## Make it your own

This skill is deliberately minimal — a working baseline that proves everything can go through the queue. Copy it into your own skills directory and grow it to match your workflow: session continuation (`resume_session_id` lets a run resume your current conversation with full context), per-run model routing (`model` on `enqueue_task`/`enqueue_chain`), worktree-vs-primary-checkout targeting rules, plan previews before enqueuing. The whole MCP surface is just five tools: `enqueue_task`, `enqueue_chain`, `run_task_definition`, `list_task_definitions`, `list_tasks`.
