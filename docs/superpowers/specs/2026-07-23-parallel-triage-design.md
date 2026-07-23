# parallel-triage — design

**Date:** 2026-07-23  
**Status:** approved for planning  
**Approach:** A — prompt-only queohoh task (report-only v1)  
**Workspace home (implementation):** `platform/tasks/parallel-triage/` under the operator’s queohoh config workspace (e.g. `~/workspace/queohoh/platform/tasks/parallel-triage/`)

## 1. Purpose

Answer: **which Linear tickets can start next as parallel work branching off `main`**, without stacking on open PRs.

- **Why:** stacked PRs create recurring merge pain; branching off `main` is preferred.
- **Tolerance:** mild conflicts with open work are OK; **hard dependency** on an unmerged PR/ticket is not.
- **v1:** report only (markdown table in the task report). No intake/autofix enqueue.
- **v2 (out of scope for implementation plan v1):** optional fan-out of `✓` + `simplicity=autofix` rows into `intake`.

## 2. Success criteria

- One on-demand TUI/MCP run produces a table the operator can act on in under a minute.
- Open PRs whose base is not `main` never appear as “start next from main” candidates; they appear as **in flight** with **stack risk**.
- Prefer false “can go” with a conflict note over false “blocked” when overlap is only soft (same files, related domain) rather than hard (work does not exist on main yet).
- Cap the primary ranked table at about **10** rows.

## 3. Inputs

| Source | What |
|--------|------|
| **Linear** | Issues assigned to the authenticated operator (viewer), all open states: Todo, Backlog, In Progress, In Review, etc. Exclude Done / Canceled / completed equivalents. |
| **GitHub** | Open PRs on `justicebid/platform` authored by the operator. |
| **Main** | `origin/main` as the preferred branch base; use PR merge state for “already landed”. |
| **Linking** | Ticket ↔ PR via `JUS-XXX` in branch name, PR title, PR body; Linear GitHub attachments when tools expose them. |

**Team / repo defaults:** Justice Bid team / `platform` project — same spirit as `intake`. Prefer `vars.yaml` / config vars (`linear_team_id`, `platform_repo`, `github_user`) when present; do not hardcode secrets.

**Scan vs table:** the agent may list more issues/PRs than 10; the **published** start-next table is capped at ~10.

## 4. Classification

### 4.1 Row kinds

| Kind | Definition | Role in report |
|------|------------|----------------|
| **Start-next candidate** | Open Linear issue with **no** open PR linked | Competes for “can branch off main” ranking |
| **In flight** | Open Linear issue with an open PR, or open PR even if ticket link is fuzzy | Context only; not a new main-branch start |
| **Landed / done** | Linked PR merged or issue Done | Omit from start-next; optional one-line “recently landed” |

### 4.2 Can-go decision (start-next candidates only)

| Outcome | Rule |
|---------|------|
| **✓ can go** | No hard dependency on unfinished work in an **open** PR or another open ticket. Soft overlap / possible merge conflict with open PRs is allowed and should be noted under Impact or Assumptions. |
| **blocked by #N** | Fully depends on open PR `#N` (stacked base, “continues #N”, API/schema only on that branch, explicit blocked-by). |
| **blocked by JUS-XXX** | Fully depends on another open ticket that has not landed on main. |

**Hard vs soft:**

- **Hard:** cannot implement a correct fix on `main` today without that other branch’s commits (missing API, shared unfinished design, explicit stack).
- **Soft:** touches same files/areas as an open PR but the ticket is independently shippable from main (operator accepts conflict resolution).

### 4.3 Stack risk (in flight)

If an open PR’s **base branch is not `main`** (and not the default trunk), mark **stack risk** and name the base (branch and/or PR if resolvable). Do not recommend stacking further work on it.

## 5. Report schema

### 5.1 Header

One line summary, e.g.:

`3 can branch off main · 2 blocked · 4 in flight`

### 5.2 Primary table (~10 rows)

Start-next candidates only, sorted: **✓ can go first**, then by **Priority** (High → Low), then Impact.

| Column | Content |
|--------|---------|
| **Ticket** | `JUS-XXX` + short title |
| **PR** | `#N` if linked, else `—` |
| **Can go** | `✓` or `blocked by #N` / `blocked by JUS-XXX` |
| **PR status** | `none` / `open` / `draft` / `merged`; if open, include base (`main` vs other) |
| **Impact** | `S` / `M` / `L` + short phrase (user/product reach) |
| **Simplicity** | `autofix` = clear, bounded, agent-safe without product Q&A; `human` = needs human judgment / Q&A cycle |
| **Priority** | `High` / `Med` / `Low` (urgency × impact × readiness to start from main) |

Use markdown (GFM table). Checkmarks may be the Unicode `✓` character.

### 5.3 In-flight section

Compact list or second table: PR `#N`, ticket if known, base branch, stack risk yes/no.

### 5.4 Assumptions

Short bullets for judgment calls (e.g. “treated JUS-1909 as soft overlap with #1890”).

### 5.5 Delivery

- **v1:** task report (+ transcript). No Slack, no Linear comments, no PR comments.
- Artifact path is the normal queohoh run report for the task instance.

## 6. Task shape (queohoh)

```yaml
# platform/tasks/parallel-triage/config.yaml (normative sketch)
description: Rank tickets that can branch off main in parallel (avoid stacks)
worktree: repo
dedup: none
model: [claude/claude-opus-4.8, grok/grok-4.5]
timeout: 30m
priority: normal
on_done: archive
# v1: no args — always "my Linear issues + my open PRs"
```

| Concern | Choice |
|---------|--------|
| Worktree | `repo` — read-only analysis on primary checkout |
| Args | none in v1 |
| Dedup | `none` (re-runnable anytime) |
| Model | judgment-heavy fallback chain (opus then grok); align labels with workspace catalog at implement time |
| Timeout | 30m |
| on_done | `archive` so successful runs leave the live queue |
| Cron | none in v1 (on-demand only) |

### 6.1 Agent constraints

- **No questions** to the operator (same autonomy as intake).
- **No product code commits**, no worktree spawn, no PR create/push.
- Prefer Linear MCP + `gh` CLI; fall back with a clear `🚧 blocked` if both identity surfaces fail.
- Never `git reset --hard`.

### 6.2 Prompt structure (implementation outline)

1. Resolve identity (Linear viewer, GitHub user from vars/`gh api user`).
2. Fetch open Linear issues assigned to me (paginate reasonably).
3. Fetch open authored PRs on platform.
4. Link tickets ↔ PRs.
5. Classify start-next / in-flight / landed.
6. Score can-go, impact, simplicity, priority for candidates.
7. Emit header + table + in-flight + assumptions.

## 7. Non-goals (v1)

- Enqueue to `intake` / `autofix` / `pr-ready`
- Deep multi-hop dependency graphs
- Auto-retarget stacked PRs onto `main`
- Merge-conflict simulation of every branch against main
- Writing Linear status or posting to Slack/GitHub
- Cron / digests

## 8. v2 hook (documentation only)

When the report is trusted:

- Optional flag or follow-up task: for rows with `Can go = ✓` and `Simplicity = autofix`, enqueue `intake` (or a thinner “start ticket worktree from main” path).
- Not part of the v1 implementation plan.

## 9. Testing / validation

- Manual: run once against the real workspace; confirm table columns and that stacked PRs only appear under in-flight with stack risk.
- No daemon/core product tests required (workspace task prompt only), unless the implementation later adds a shared script under `platform/shared/`.

## 10. Implementation order (for writing-plans)

1. Add `platform/tasks/parallel-triage/config.yaml` + `prompt.md` in the config workspace.
2. Wire any missing vars documentation in workspace README / setup notes if identity vars are required.
3. Dry-run from TUI; tune prompt if “blocked” is over-eager.
4. Stop (no fan-out).

## 11. Decisions log

| Decision | Choice |
|----------|--------|
| Name | `parallel-triage` |
| Approach | Prompt-only task (A) |
| Ticket set | All open Linear issues assigned to me (todo + backlog + in progress + …) |
| PRs | All open PRs authored by me |
| In-flight tickets | Context rows, not start-next candidates |
| Can-go rule | Hard dep only; soft conflict OK |
| Delivery | On-demand task report |
| v1 fan-out | No |
