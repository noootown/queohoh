---
name: "qoo"
description: Queue work onto the queohoh orchestrator — the single skill interface to the queue. By default queues a headless CONTINUATION of the current session in the current worktree, so hours-long work runs in the background while you close the tab. Also routes to project task definitions (e.g. pr-ready). Requires the queohoh MCP server and daemon.
user-invocable: true
argument-hint: "<what you want done> | status"
---

# /qoo — queue it and close the tab

Turn the user's request into a queued queohoh run. The daemon runs it
headless in the right worktree; by default the run RESUMES the current
session, so the full conversation context carries over. Your job is ONLY
to route and enqueue — never do the work yourself.

**Input:** `$ARGUMENTS` — free text describing the work, or exactly `status`.

## Routing

- `status` (single token) → call `list_tasks`, render a compact table
  (id-suffix, status, repo:worktree, first ~60 chars of prompt), done.
- Anything else → the Enqueue procedure.

## Enqueue procedure

### 1. Resolve context

In one Bash call, gather:

- `TOPLEVEL=$(git rev-parse --show-toplevel)` — the current worktree.
- `DIRTY=$(git status --porcelain)` — pre-existing uncommitted changes.
- Your own session ID AND launch dir, both from your scratchpad path
  (system prompt: `.../<MUNGED-LAUNCH-DIR>/<SESSION-UUID>/scratchpad`).
  Take the last two path segments before `scratchpad`: the UUID is the
  session id, and the segment before it is the munged launch dir.
  Verify `~/.claude/projects/<munged-launch-dir>/<uuid>.jsonl` exists;
  if it is missing, STOP and tell the user — never guess another
  session. Then munge $TOPLEVEL the same way (every non-alphanumeric
  character → `-`) and compare it to the munged launch dir. If they
  differ, this session was launched in a subdirectory of the worktree
  (or elsewhere): the worker resumes from the worktree root, so
  `--resume` would not find this session. STOP and explain that
  continuation requires the session to have been launched at the
  worktree root; offer to queue a fresh (non-resuming) task instead.

You also know your own model ID (e.g. `claude-fable-5`) — pass it as
`model` so the resumed run doesn't downgrade to the daemon default.

### 2. Match against task definitions

Call `list_task_definitions` and match the request by meaning, not string
equality:

- Exactly one definition plausibly fits → use it. If its args or mode are
  genuinely ambiguous, ask ONE short question (e.g. "/qoo pr-ready" →
  which mode?), then
  `run_task_definition(repo, name, args?, cwd=$TOPLEVEL, resume_session_id=<id>)`.
- 2+ fit → ask the user to pick (one short question).
- None fit → ad-hoc enqueue (step 3).
- Never invent definition names — only names returned by
  `list_task_definitions`.

### 3. Ad-hoc: compose the handoff prompt

The run resumes this session — it already has the full conversation. The
prompt is a short directive, not a context dump. It is not limited to
implementation work; match it to whatever was agreed (implement, make a
PR ready, write docs, run an audit, ...):

- Restate the concrete deliverable and its done-condition as agreed in
  the conversation. Fold the user's /qoo arguments in verbatim.
- Add verification steps only if the task has a runnable check (tests,
  build, lint).
- If $DIRTY was non-empty, add: "The tree already has uncommitted
  changes; fold them into your commits or commit them separately first."
- Always end with: "Commit all work when done — a run that leaves the
  tree dirty is marked failed."

Then:
`enqueue_task(prompt, cwd=$TOPLEVEL, resume_session_id=<id>, model=<your model id>)`.

### Session mode

ALWAYS continuation — for definition runs too — unless the user
explicitly says "fresh". For a fresh run, omit `resume_session_id` and
make the prompt fully self-contained: transcribe verbatim error messages,
file paths, stack traces, and a faithful description of any pasted images
(a fresh worker sees ONLY the prompt).

## Report

One line: what was queued, target repo:worktree, "resumes this session",
model. Then two short notes:

- The run starts once this session goes idle (~5 min after the last
  prompt) or the tab is closed; continuing to work in this session
  afterwards diverges from the fork point.
- Progress is visible in the queohoh TUI.

## Failure modes

- MCP tools missing or connection refused → the daemon isn't running:
  `queohoh daemon` (foreground) or `queohoh launchd:install`.
- Enqueue error "no registered project contains ..." → relay the error's
  config.yaml snippet to the user verbatim; do not try to work around it.
- `run_task_definition` timed out → call `list_tasks` to check whether
  the tasks were created before telling the user anything failed.

## Rules

- Never implement the work inline — even if it looks quick. The point of
  the queue is that the user stays in flow.
- One clarifying question max; prefer sensible defaults.
