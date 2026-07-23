# parallel-triage Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an on-demand, report-only queohoh task `parallel-triage` that ranks the operator’s Linear tickets for parallel starts off `main` (avoid stacked PRs).

**Architecture:** Prompt-only task under the config workspace (`platform/tasks/parallel-triage/`), same shape as `intake` / `mail-check`: `config.yaml` + `prompt.md`, `worktree: repo`. The agent loads Linear + GitHub via MCP/`gh`, classifies hard vs soft dependencies, and writes a GFM report table. No product daemon/core changes.

**Tech Stack:** queohoh task definitions (YAML frontmatter + markdown prompt), Linear MCP, GitHub CLI (`gh`), vars from `platform/vars.yaml`.

**Spec:** `docs/superpowers/specs/2026-07-23-parallel-triage-design.md`

## Global Constraints

- **v1 is report-only** — never enqueue `intake` / `autofix` / `pr-ready`.
- **No product commits**, no worktree spawn, no PR create/push, never `git reset --hard`.
- **No operator questions** — autonomous judgment; list assumptions in the report.
- **Can go** = no hard dependency on open PR/ticket; soft file overlap is allowed.
- **In-flight** tickets (open PR linked) are context only, not start-next candidates.
- **Stack risk** when open PR base ≠ `main`.
- Primary table **~10 rows**; sort ✓ first, then priority High→Low.
- Task lives in **config workspace** path below (not the queohoh product repo).
- Model labels follow existing platform tasks: `claude/opus` then `grok/grok-4.5`.

**Paths (absolute base):**

| Role | Path |
|------|------|
| Config workspace | `/Users/noootown/workspace/queohoh` |
| Platform tasks | `/Users/noootown/workspace/queohoh/platform/tasks/` |
| New task dir | `/Users/noootown/workspace/queohoh/platform/tasks/parallel-triage/` |
| Platform vars | `/Users/noootown/workspace/queohoh/platform/vars.yaml` |
| Reference task | `/Users/noootown/workspace/queohoh/platform/tasks/intake/` |
| Spec (product repo) | `/Users/noootown/Downloads/personal/queohoh/docs/superpowers/specs/2026-07-23-parallel-triage-design.md` |

---

### Task 1: Create `parallel-triage` task definition

**Files:**
- Create: `/Users/noootown/workspace/queohoh/platform/tasks/parallel-triage/config.yaml`
- Create: `/Users/noootown/workspace/queohoh/platform/tasks/parallel-triage/prompt.md`
- Reference (read only): `/Users/noootown/workspace/queohoh/platform/tasks/intake/config.yaml`
- Reference (read only): `/Users/noootown/workspace/queohoh/platform/vars.yaml`

**Interfaces:**
- Consumes: template vars `{{platform_repo}}`, `{{linear_team_id}}`, `{{linear_workspace}}`, `{{github_username}}` (and any global builtins already available to prompts).
- Produces: queohoh definition `platform/parallel-triage` runnable via TUI Tasks pane or `run_task_definition` / MCP with `repo: platform`, `name: parallel-triage`, no args.

- [ ] **Step 1: Confirm parent dir and vars exist**

```bash
ls /Users/noootown/workspace/queohoh/platform/tasks/intake/config.yaml
test -f /Users/noootown/workspace/queohoh/platform/vars.yaml && rg -n 'linear_team_id|platform_repo|github_username' /Users/noootown/workspace/queohoh/platform/vars.yaml
test ! -e /Users/noootown/workspace/queohoh/platform/tasks/parallel-triage
```

Expected: intake config exists; vars include `linear_team_id: JUS`, `platform_repo: justicebid/platform`, `github_username: ianchiu-jb`; `parallel-triage` does not exist yet.

- [ ] **Step 2: Write `config.yaml`**

Create `/Users/noootown/workspace/queohoh/platform/tasks/parallel-triage/config.yaml` with **exactly** this content (adjust only if catalog rejects `claude/opus` — then use the same model line as `intake`):

```yaml
# parallel-triage — rank Linear tickets that can branch off main in parallel.
#
# Report-only: loads the operator's open Linear issues + open GitHub PRs on
# platform, classifies hard dependencies on unmerged work, and prints a table
# of what can start from main without stacking. Does NOT enqueue intake/autofix.
#
# Spec: queohoh docs/superpowers/specs/2026-07-23-parallel-triage-design.md
description: Rank tickets that can branch off main in parallel (avoid stacks)
worktree: repo
dedup: none
model: [claude/opus, grok/grok-4.5]
timeout: 30m
priority: normal
on_done: archive
```

- [ ] **Step 3: Write `prompt.md`**

Create `/Users/noootown/workspace/queohoh/platform/tasks/parallel-triage/prompt.md` with the full prompt below (copy verbatim into the file):

```markdown
You are **parallel-triage** for {{platform_repo}}. Produce a short, actionable report: which of **my** Linear tickets can start **next as parallel work branching off `main`**, without stacking on open PRs.

You never ask the operator a question. Make every call yourself; list judgment calls under **Assumptions**.

## Mission (one run)

1. Load **all open Linear issues assigned to me** (Todo, Backlog, In Progress, In Review, and any other open state — not Done/Canceled).
2. Load **my open GitHub PRs** on `{{platform_repo}}`.
3. Link tickets ↔ PRs (`JUS-XXX` in branch/title/body; Linear GitHub links if available).
4. Classify each item: **start-next candidate** vs **in flight** vs landed/done.
5. For start-next candidates: decide **✓ can go** vs **blocked by #N / JUS-XXX** (hard dependency only).
6. Score **Impact**, **Simplicity**, **Priority**.
7. Print the report (header + table + in-flight + assumptions). **Stop.** Do not implement, commit, open PRs, or enqueue other tasks.

## Autonomy + hard rules

- **No questions.**
- **Report only.** Do **not** call `enqueue_task`, `enqueue_chain`, `run_task_definition`, or intake/autofix.
- **No product code changes.** No commits that change application code. No `git reset --hard`. No worktree create/delete.
- Prefer **Linear MCP** for issues; prefer **`gh`** for PRs. If identity or tools fail, stop with `🚧 blocked — <reason>` rather than inventing tickets/PRs.
- Team key / workspace: `{{linear_team_id}}` / `{{linear_workspace}}`. GitHub login hint: `{{github_username}}` (still confirm via `gh api user` when possible).

## Step 1 — Identity

1. Resolve Linear **viewer** (me) and confirm assignee filtering uses that user.
2. Resolve GitHub login (`gh api user -q .login` or `{{github_username}}`).
3. Confirm repo is `{{platform_repo}}`.

If either Linear or GitHub identity cannot be established → `🚧 blocked` and stop.

## Step 2 — Fetch Linear issues

Fetch open issues **assigned to me** on the Justice Bid team (`{{linear_team_id}}`). Include Todo, Backlog, In Progress, In Review, and other open states. Exclude Done, Canceled, and completed equivalents.

Capture per issue: identifier (`JUS-XXX`), title, state/status name, priority if present, url, description snippet (enough to judge deps), any explicit blocked-by / related links.

Paginate reasonably (do not stop at 10 issues for the fetch — the **table** is capped later).

## Step 3 — Fetch open PRs

```bash
gh pr list --repo {{platform_repo}} --author @me --state open --limit 50 \
  --json number,title,url,isDraft,baseRefName,headRefName,body
```

If `--author @me` fails, use the resolved login from Step 1.

Capture: number, title, draft?, base branch, head branch, body (for `JUS-XXX` links).

Optional context (best-effort, do not block on failure):

```bash
git -C "{{repo_path}}" fetch origin main 2>/dev/null || true
```

(`repo_path` = primary checkout; you are already on `worktree: repo`.)

## Step 4 — Link tickets ↔ PRs

For each open PR, extract ticket ids from head branch, title, and body (`JUS-\\d+` case-insensitive). Map PR → ticket(s) and ticket → open PR(s).

## Step 5 — Classify

| Kind | Definition | Report role |
|------|------------|-------------|
| **Start-next candidate** | Open Linear issue with **no** open PR linked | Primary table |
| **In flight** | Open issue with open PR, or open PR with/without clear ticket | In-flight section only |
| **Landed / done** | Issue Done or only-linked PRs are merged | Omit from start-next (optional one-line note) |

### Can-go (start-next candidates only)

| Outcome | When |
|---------|------|
| **✓** | No **hard** dependency on unfinished work in an open PR or another open ticket. Soft overlap OK. |
| **blocked by #N** | Fully depends on open PR `#N` (stacked base, “continues #N”, API only on that branch, explicit blocked-by). |
| **blocked by JUS-XXX** | Fully depends on another open ticket not on main. |

**Hard vs soft:**

- **Hard:** cannot implement correctly on current `main` without that other branch’s commits.
- **Soft:** same files/domain as an open PR but still shippable from main (operator accepts conflict). Prefer **✓** with a conflict note over false blocked.

### Stack risk (in flight)

If open PR `baseRefName` is not `main` (and not the repo default trunk), mark **stack risk** and name the base.

## Step 6 — Score start-next candidates

- **Impact:** `S` / `M` / `L` + short phrase (user/product reach).
- **Simplicity:** `autofix` if clear, bounded, agent-safe without product Q&A; else `human`.
- **Priority:** `High` / `Med` / `Low` from urgency × impact × readiness to start from main.

## Step 7 — Emit the report

### Header (required)

One line, e.g.:

`3 can branch off main · 2 blocked · 4 in flight`

### Primary table (required, max ~10 rows)

**Only start-next candidates.** Sort: **✓ first**, then Priority High→Med→Low, then Impact L→S.

| Ticket | PR | Can go | PR status | Impact | Simplicity | Priority |
|--------|-----|--------|-----------|--------|------------|----------|
| JUS-XXX short title | — or #N | ✓ or blocked by #N / JUS-YYY | none / open (base: main) / draft / … | M — phrase | autofix or human | High |

Use Unicode `✓` for can-go. Use `—` when no PR.

If more than 10 candidates, keep the best 10 by the sort order; mention how many were omitted in Assumptions.

### In flight (required if any)

Compact table or list:

| PR | Ticket | Base | Stack risk |
|----|--------|------|------------|
| #N | JUS-XXX or — | main or other-branch | yes/no (+ base if yes) |

### Assumptions (required)

Bullets for judgment calls (e.g. “JUS-1909 soft overlap with #1890 — can go”).

### Stop

Do not enqueue follow-up work. End the run after the report.

## Failure modes

- No Linear access → `🚧 blocked — cannot list Linear issues: <reason>`
- No GitHub access → `🚧 blocked — cannot list PRs: <reason>`
- Zero open issues and zero open PRs → still emit a valid empty report (`0 can branch off main · 0 blocked · 0 in flight`) and say so.
```

- [ ] **Step 4: Verify files on disk**

```bash
ls -la /Users/noootown/workspace/queohoh/platform/tasks/parallel-triage/
wc -l /Users/noootown/workspace/queohoh/platform/tasks/parallel-triage/config.yaml \
      /Users/noootown/workspace/queohoh/platform/tasks/parallel-triage/prompt.md
head -20 /Users/noootown/workspace/queohoh/platform/tasks/parallel-triage/config.yaml
```

Expected: both files present; config shows `worktree: repo`, `on_done: archive`, no `args:`.

- [ ] **Step 5: Commit in the config workspace** (if that tree is a git repo)

```bash
cd /Users/noootown/workspace/queohoh
git rev-parse --git-dir 2>/dev/null && \
  git add platform/tasks/parallel-triage && \
  git status -sb && \
  git commit -m "feat(platform): add parallel-triage task (report-only)"
```

If `/Users/noootown/workspace/queohoh` is **not** a git repo, skip commit and note “files only” in the handoff. Do **not** commit these files into the queohoh product repo.

---

### Task 2: Dry-run validation from queohoh

**Files:**
- None created (runtime validation only)
- Uses: daemon + TUI / MCP against definition `platform/parallel-triage`

**Interfaces:**
- Consumes: Task 1 definition on disk (daemon reloads defs from workspace)
- Produces: one completed/archived task with a report containing the required sections

- [ ] **Step 1: Ensure daemon sees the new definition**

Reload/restart daemon if hot-reload does not pick up new task dirs (operator usual path, e.g. `mise run daemon` or existing `daemon-ensure`).

```bash
# From a shell with QUEOHOH_WORKSPACE pointing at the config workspace:
# list definitions should include parallel-triage under platform
# Example if qoo CLI / MCP is available:
#   use MCP list_task_definitions or TUI TASKS pane filter "parallel"
```

Expected: `parallel-triage` appears for project `platform`.

- [ ] **Step 2: Run the task once (no args)**

From TUI: select `parallel-triage` → Run.  
Or MCP: `run_task_definition` with `repo: "platform"`, `name: "parallel-triage"`, `args: []`.

Expected: task reaches `done` (or `verify-failed` only if a verify is later added — v1 has no verify); `on_done: archive` soft-dismisses success.

- [ ] **Step 3: Check report content**

Open the task **report** (or transcript tail). Confirm:

1. Header line with counts (`can branch off main` / `blocked` / `in flight`)
2. GFM table with columns: Ticket, PR, Can go, PR status, Impact, Simplicity, Priority
3. At most ~10 start-next rows
4. In-flight section lists open PRs; any non-`main` base marked stack risk
5. Assumptions section present
6. No evidence of enqueue_chain / product commits

- [ ] **Step 4: Tune only if blocked is over-eager**

If the first run marks soft overlaps as blocked, edit **only** the hard-vs-soft bullets in `prompt.md` to restate “prefer ✓ with conflict note”. Re-run once. Do not add fan-out logic.

- [ ] **Step 5: Record validation outcome**

Leave a short note in the PR/commit message or operator chat: “dry-run OK” or “prompt tweak: …”. No automated test file required for v1.

---

### Task 3: Optional index pointer (docs)

**Files:**
- Modify (only if the workspace already indexes tasks): `/Users/noootown/workspace/queohoh/platform/tasks/README.md` **if it exists**
- Else skip this task entirely

- [ ] **Step 1: Check for tasks README**

```bash
test -f /Users/noootown/workspace/queohoh/platform/tasks/README.md && echo exists || echo skip
```

- [ ] **Step 2: If exists, add one bullet**

```markdown
- `parallel-triage` — report-only: which tickets can branch off main in parallel (avoid stacks)
```

- [ ] **Step 3: Commit in config workspace if applicable**

```bash
cd /Users/noootown/workspace/queohoh
git add platform/tasks/README.md
git commit -m "docs(platform): mention parallel-triage in tasks README"
```

---

## Spec coverage (self-review)

| Spec section | Plan task |
|--------------|-----------|
| Purpose / report-only v1 | Task 1 prompt mission + hard rules |
| Inputs Linear + GH + linking | Task 1 Steps 2–4 |
| Classification can-go / in-flight / stack risk | Task 1 Step 5 |
| Report schema | Task 1 Step 7 |
| Task shape config | Task 1 config.yaml |
| Non-goals / no fan-out | Global constraints + prompt |
| Manual validation | Task 2 |
| v2 hook | Not implemented (documented in prompt “Stop”) |

**Placeholders:** none.  
**Type consistency:** n/a (no code types).  
**YAGNI:** no discover script, no cron, no daemon changes.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-07-23-parallel-triage.md`.

**Two execution options:**

1. **Subagent-Driven (recommended)** — fresh subagent per task, review between tasks  
2. **Inline Execution** — implement in this session with executing-plans checkpoints  

Which approach?
