# Task lifecycle: live, archive, purge

queohoh keeps a **track record** of work, but the queue pane should stay
focused on what still matters. Lifecycle has three layers:

| Layer | Meaning | Storage |
| --- | --- | --- |
| **Live queue** | Active or still-relevant work | `~/.local/state/queohoh/tasks/` |
| **Archive** | Soft dismiss — “I don’t care right now,” still recoverable history | `…/archive/` |
| **Purge** | Hard cleanup — file removed; no longer on the board | gone |

**Archive is not a permanent museum.** It is the list of things you soft-dismissed.
**Purge** is when that record’s life ends.

```
                    ┌─────────────┐
   create/enqueue → │    LIVE     │ ← running / queued / failed / etc.
                    └──────┬──────┘
           on_done:archive │  or human [a]rchive
                           ▼
                    ┌─────────────┐
                    │   ARCHIVE   │  track record (dimmed in TUI)
                    └──────┬──────┘
                           │
        worktree removed   │   purge_after_days
        (bound tasks)      │   (any terminal task)
                           ▼
                       ┌───────┐
                       │ PURGE │  deleted from disk
                       └───────┘
```

---

## After a run finishes

Terminal statuses: `done`, `failed`, `cancelled`, `verify-failed`, `skipped`.

### Soft path — `on_done` (definition only)

Set on a task definition’s `config.yaml`:

```yaml
on_done: stay     # default — leave successful runs on the live list
on_done: archive  # on success only, move to archive immediately
```

| Outcome | Behavior |
| --- | --- |
| **`done`** + `on_done: stay` | Stays **live** until human archives or purge |
| **`done`** + `on_done: archive` | **Archived** as soon as the run succeeds |
| **failed / cancelled / verify-failed / skipped** | Always stay **live** until human archive or purge (so problems remain visible) |

Legacy alias: `archive_on_done: true` → treated as `on_done: archive`.

Ad-hoc tasks (no definition) always behave like `on_done: stay`.

### Manual archive

In the TUI, archive moves a terminal row live → archive. Unarchive reverses it.
That is the same soft-dismiss as `on_done: archive`, just human-driven.

---

## Purge — hard cleanup

### 1. Worktree removed → purge

When a worktree disappears (TUI remove, external `git worktree remove`, etc.):

1. **Cancel** live non-terminal work on that worktree (queued / needs-input /
   stop running).
2. **Purge** every task that targeted that worktree — live **or** archived.

The lane is gone; the board should not keep its history. This is independent of
`on_done` and of age.

**`@repo` / primary checkout never goes away**, so worktree purge never fires
for main-checkout tasks. Those need `purge_after_days` (below).

### Automatic worktree cleanup

`worktree_retention_days: null` (the default, also used when omitted from global
config) disables automatic removal of persistent worktrees. A positive integer
enables removal after that many idle days, measured from the newest task
completion or commit. A project's `vars.yaml` can override the global value;
explicit `null` disables it for that project, while an omitted key inherits.
Ephemeral worktrees from `temp` tasks have no age threshold.

When persistent cleanup is enabled, the newest terminal task record for each
present worktree is retained as activity evidence even past `purge_after_days`.
Older records still expire normally. The retained record goes away when the
worktree is removed, or becomes subject to normal task expiry when worktree
retention is disabled. This also leaves one record for a worktree that fails
the ownership check; it does not grant permission to delete that worktree.

Automatic cleanup requires a creation receipt recorded when queohoh successfully
creates a new branch and worktree. Old worktrees, manually created directories,
and adopted PR branches are left alone. The receipt must still match the exact
repository, directory and Git metadata. Naming a directory `repo.branch` does
not make it eligible.

Both kinds of automatic cleanup require terminal, unfavorited task history,
no active registered sessions, and no worktree protection or favorite. Fresh
checks must find no tracked edits, untracked or ignored files, and no open PR.
Failed Git/PR checks stop cleanup. While deletion is in progress, queue writes
targeting that worktree return a retry message. Cleanup runs in the background
and uses non-force Git removal; it preserves the branch and its commits.

Manual removal remains an explicit destructive action. If its background
deletion leaves a directory behind, queohoh reports it for manual inspection;
it never recursively deletes an unrecognized leftover or a path extracted
from an error message.

### 2. Age — `purge_after_days`

Hard-delete **terminal** tasks after N days, whether they sit on **live** or
**archive**.

| Clock | `finished_at`, falling back to `created` if missing |
| --- | --- |
| Statuses | All terminal (`done`, `failed`, `cancelled`, `verify-failed`, `skipped`) |
| Non-terminal | Never age-purged (a long running task is safe) |

**Precedence:**

1. Task stamp (from the definition at create time)
2. Live definition lookup (`purge_after_days` on the def — config edits apply)
3. Workspace global `purge_after_days` (default **14**)

```yaml
# config.yaml (workspace)
purge_after_days: 14
```

```yaml
# tasks/mail-check/config.yaml — override for a noisy cron
on_done: archive
purge_after_days: 1
```

Ad-hoc tasks use **only** the global value (no def to override).

Legacy: `archive_after_days` in config is still accepted as a fallback if
`purge_after_days` is absent. Def-level `task_retention_days` maps to
`purge_after_days` when the new key is missing (old “soft age-archive” is gone;
age only hard-purges now).

---

## Favorites

A favorited task is user-pinned and gets four exceptions to the flow above:

1. **Manual archive/dismiss refuses.** Archiving (or the terminal half of
   `[a]rchive`/skip-as-dismiss) a favorited task throws “task is favorited —
   unfavorite it first” — but only while its target worktree still exists. The
   guard lifts once that worktree is gone (an orphaned favorite has nowhere
   live left to protect).
2. **`on_done: archive` is skipped.** A favorited task that finishes `done`
   stays live even when its definition says `on_done: archive` — the pin
   outranks the def's soft-dismiss.
3. **Age purge skips it.** `purge_after_days` never hard-deletes a favorited
   terminal task, live or archived.
4. **Worktree-removed purge does NOT skip it.** Deleting a worktree still
   purges every task bound to it, favorited or not — this is a deliberate
   design decision, not an oversight: the lane is gone, so its history goes
   with it regardless of the pin.

One more wrinkle: the archived-tail wire cap (newest 200, see below) means a
favorited archived task older than that is still protected from purge but
won't be visible in the TUI.

---

## Recommended patterns

| Kind of work | Suggested config |
| --- | --- |
| PR / worktree feature work | default (`on_done: stay`); worktree remove purges; global 14d backstop |
| Noisy `@repo` cron (mail-check, react bots) | `on_done: archive` + short `purge_after_days: 1` (or 3–7) |
| Important ops on main checkout | `on_done: stay` (or archive) + rely on global 14, or set def purge explicitly |
| Ad-hoc enqueue | global `purge_after_days` only — set global to avoid infinite live clutter |

---

## TUI notes

- **Live** finished rows show in the FINISHED section of the queue.
- **Archived** rows are dimmed; newest ~200 are on the wire (`ARCHIVED_WIRE_MAX`).
- Rows whose worktree was deleted used to be **hidden** in the TUI even while
  files remained; the engine now **purges** those tasks so hide-vs-delete is not
  a second mental model.

---

## Definition config cheat sheet

```yaml
# optional — default stay
on_done: archive

# optional — default = workspace purge_after_days (14)
purge_after_days: 1
```

Workspace:

```yaml
purge_after_days: 14
```

See also: `docs/setup.md` (install/config), `AGENTS.md` (architecture),
`packages/daemon/AGENTS.md` (daemon invariants).
