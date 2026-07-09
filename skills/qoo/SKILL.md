---
name: "qoo"
description: Queue work onto the queohoh orchestrator. Describe what you want ("review Kevin's auth PR", "fix the flaky date test in platform") and this skill finds the matching task definition and queues it — or enqueues an ad-hoc task if nothing matches. Requires the queohoh MCP server.
user-invocable: true
argument-hint: "<what you want done> | status"
---

# /qoo — queue it and forget it

Turn the user's natural-language request into a queued queohoh task. The
daemon runs it end-to-end in the right worktree; the user monitors via the
queohoh TUI. Your job is ONLY to route and enqueue — never do the work
yourself.

**Input:** `$ARGUMENTS` — free text describing the work, or exactly `status`.

## Routing

- `status` (single token) → call `list_tasks`, render a compact table
  (id-suffix, status, target repo/worktree, first ~60 chars of prompt), done.
- Anything else → the Enqueue procedure.

## Enqueue procedure

1. Call `list_task_definitions`.
2. Match the request against definitions by name and argument shape.
   Examples: "review PR 257 in platform" → `platform/pr-review` with
   args ["257"]; "run pr-review" (no args) → discovery mode.
   - Match on meaning, not string equality. If exactly one definition
     plausibly fits, use it.
   - If 2+ fit, ask the user to pick (one short question).
   - If none fit, fall back to `enqueue_task` (ad-hoc).
3. Extract the target:
   - Definition match → `run_task_definition(repo, name, args?)`.
   - Ad-hoc → `enqueue_task(prompt, repo, ref?, priority?)`. Derive `ref`
     when obvious: "PR 257" → `pr:257`, "JUS-1423" → `ticket:JUS-1423`,
     a named worktree → `worktree:<name>`; otherwise omit (defaults to a
     temp worktree). Derive `repo` from the definition list's repo names
     or the current directory; ask if genuinely ambiguous.
4. **Ad-hoc prompt quality:** the worker is a fresh agent that sees ONLY
   the prompt text. Transcribe into it: verbatim error messages, file
   paths, stack traces, and a faithful description of any pasted images.
   The prompt must stand alone.
5. Report back in one line: what was queued, where it will run, and that
   the TUI shows progress. If `run_task_definition` timed out, call
   `list_tasks` to check whether the tasks were created before telling
   the user anything failed.

## Rules

- Never implement the work inline — even if it looks quick. The point of
  the queue is that the user stays in flow.
- Never invent definition names — only use names returned by
  `list_task_definitions`.
- One clarifying question max; prefer sensible defaults.
