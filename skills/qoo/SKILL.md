---
name: "qoo"
description: Queue work onto the queohoh orchestrator — the single skill interface to the queue. From a worktree it queues a headless CONTINUATION of the current session in that worktree; from the primary checkout it always targets a new/other worktree (fresh session). Multi-step requests ("A, then B") become chains. Also routes to project task definitions (e.g. pr-ready). Requires the queohoh MCP server and daemon.
user-invocable: true
model: fable
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
- `PRIMARY`: whether this is the repo's PRIMARY checkout — a linked
  worktree has a `.git` FILE at its root, the primary has a `.git`
  directory: `[ -d "$TOPLEVEL/.git" ] && PRIMARY=yes`.
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

### 1b. Pick the target worktree (launch location decides)

The LAUNCH LOCATION decides the target, not the request's wording:

- **Non-primary worktree** → the work runs HERE, always:
  `cwd=$TOPLEVEL`, continuation by default. You are standing in the
  context; never spawn a new worktree from inside one.
- **Primary checkout** (`PRIMARY=yes`) → the work NEVER runs here (task
  work must not occupy the main branch). Target, in order:
  1. The user explicitly named a worktree, and it exists → `ref:
     worktree:<name>`.
  2. The request's DELIVERABLE is a PR or ticket — the work lands ON it
     ("fix PR #123", "resolve the conflicts on <url>", "implement
     JUS-999") → `ref: pr:<N>` / `ref: ticket:<ID>` (the daemon reuses a
     matching worktree or spawns one named after the anchor).
     A PR/ticket mentioned only as REFERENCE or input ("check out #123
     for how they did it, then add ...", "like in <url>") is NOT a
     target — keep the URL in the prompt text and fall through to temp.
     The test: would the resulting commits belong on that PR/ticket's
     branch? Genuinely ambiguous → this is a legitimate use of the one
     clarifying question ("work on PR #123's branch, or fresh
     worktree?").
  3. Otherwise → `ref: temp` (fresh throwaway worktree).
  When a definition run's args contain a reference-only PR/ticket URL,
  pass your decided `ref` explicitly — some definitions use `worktree:
  auto`, which would otherwise extract that URL from the args and target
  its branch. Your judged ref always overrides the definition's.
  Omit `cwd` entirely when using `ref`. A new/other worktree cannot
  resume this session → the run is FRESH: compose a fully self-contained
  prompt (see Session mode).
- Explicit user words always win over both rules ("here", "in this
  worktree", "new worktree", "fresh", a named worktree).

### 2. Match against task definitions

Call `list_task_definitions` and match the request by meaning, not string
equality:

- Exactly one definition plausibly fits → use it. If its args or mode are
  genuinely ambiguous, ask ONE short question (e.g. "/qoo pr-ready" →
  which mode?), then `run_task_definition(repo, name, args?, ...)` with
  the target from step 1b (`cwd=$TOPLEVEL` + `resume_session_id` from a
  non-primary worktree; ref-derived target, no resume, from primary).
- 2+ fit as SEQUENTIAL steps ("A, then B" / "and after that") → this is
  a CHAIN, not a pick: `enqueue_chain(steps=[{definition, args}, ...])`
  with the step-1b target. Chain members run FIFO in ONE worktree; a
  failed step skips the rest. Steps the definitions don't cover become
  `{prompt}` steps.
- 2+ fit as ALTERNATIVES → ask the user to pick (one short question).
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

Then `enqueue_task(prompt, model=<your model id>, ...)` with the step-1b
target: `cwd=$TOPLEVEL` + `resume_session_id=<id>` from a non-primary
worktree; `ref=<derived>`, no cwd, no resume, from the primary checkout.

### Session mode

Follows the step-1b target. Non-primary worktree → ALWAYS continuation —
for definition runs too — unless the user explicitly says "fresh".
Primary checkout → always fresh (a new/other worktree cannot resume this
session). For any fresh run, omit `resume_session_id` and make the
prompt fully self-contained: transcribe verbatim error messages, file
paths, stack traces, and a faithful description of any pasted images (a
fresh worker sees ONLY the prompt).

## Report

One line: what was queued (chains: the steps in order), target
repo:worktree (or "fresh temp worktree" / the ref), "resumes this
session" or "fresh session", model. Then two short notes:

- The run starts IMMEDIATELY (within seconds — interactive sessions no
  longer hold the lane). A continuation forks this session's transcript
  at enqueue time: anything you do in this session afterwards is NOT
  seen by the worker. Enqueue as your last action, then step away.
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
