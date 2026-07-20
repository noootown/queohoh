# queohoh setup

## 1. Build & install

```bash
pnpm install && pnpm -r build
# expose the CLI (pick one):
pnpm -F @queohoh/daemon link --global   # or add packages/daemon/dist/cli.js to PATH
```

## 2. Configure

### Discovery (env-only)

```bash
export QUEOHOH_WORKSPACE=~/path/to/your-config-workspace
# optional:
# export QUEOHOH_CONFIG=/path/to/config.yaml   # explicit file override
# export QUEOHOH_STATE_DIR=~/.local/state/queohoh
```

The daemon loads **`$QUEOHOH_WORKSPACE/config.yaml`**. Put that export in your shell profile (e.g. `path.zsh`) so interactive shells, the TUI, and `daemon-ensure` all agree. `launchd:install` snapshots the same env into the plist so a reboot still finds the workspace.

If neither `QUEOHOH_WORKSPACE` nor `QUEOHOH_CONFIG` is set, the daemon falls back to `~/.config/queohoh/config.yaml` (legacy).

### Config file

Created with comments on first daemon start if missing:

```yaml
# $QUEOHOH_WORKSPACE/config.yaml
workspace: .   # or an absolute path to the config workspace root
projects:
  - name: my-app
    path: ~/code/my-app
max_concurrent_tasks: 3
archive_after_days: 7
vars:
  github_user: you
```

Task definitions live in the workspace, one directory per project: `$QUEOHOH_WORKSPACE/<project>/tasks/<name>/` (`config.yaml` + `prompt.md`). An optional `$QUEOHOH_WORKSPACE/<project>/vars.yaml` supplies per-project template vars.

`vars.yaml` also holds two reserved keys that are read as settings rather than exposed as `{{var}}` placeholders:

- `default_models:` — an ordered fallback list of `provider/label` model refs (e.g. `[claude/claude-opus-4.8, grok/grok-4.5]`) that overrides the global `default_models:` for this project's tasks/defs that don't set their own `model:`.
- `github_id:` — your author identity, e.g. `github_id: noootown`. The TUI uses it to sort your own worktrees first. A worktree counts as **yours** when `github_id` is a case-insensitive **substring of the last-commit author email**, OR a case-insensitive **substring of the author name**. So pick a value that appears in the email or name of the commits you author — e.g. the login embedded in a GitHub noreply email (`12345+noootown@users.noreply.github.com` → `noootown`), or a distinctive token of your name/email if you commit as `Ian Chiu <noootown@gmail.com>` (here `noootown` matches the email, `Ian` or `Chiu` matches the name; your work GitHub login would match neither). Optional and parsed leniently — an absent, empty, or non-string value simply disables the "mine-first" sort.

### Builtin vars

Prompts, `pre_run`/`post_run` hooks, and the `verify` command (see below) can reference these `{{var}}` placeholders without declaring them as args. Any explicitly configured var (global `vars`, project `vars.yaml`, or an arg of the same name) overrides the builtin.

Resolved at **instantiate time** (definition → task):

- `{{project}}` — the registered project name (e.g. `platform`).
- `{{repo_path}}` — the project's primary-checkout path from `config.yaml`.

Resolved at **execution time** (in the task's actual worktree), via a second render pass — so they work even for late-resolving refs (`pr:`, `ticket:`, `temp`):

- `{{worktree}}` — the resolved worktree/lane name.
- `{{worktree_path}}` — its absolute path.
- `{{branch}}` — `git rev-parse --abbrev-ref HEAD` in that worktree (empty if it can't be read).
- `{{ticket}}` — the ticket id derived from the branch name (convention: the branch is named after its ticket, so `jus-1008-fix-thing` → `JUS-1008`; empty when the branch has no ticket-shaped token).

### Done conditions (`verify`)

Headless workers confidently report success. A `verify` command lets the
**framework** — not the worker — decide whether a task really succeeded. After
the worker exits claiming success and the tree is clean, the daemon runs
`verify` in the task's worktree (10-minute cap). Exit `0` → the task is `done`
and records `verified: true`; a non-zero exit or a timeout lands the task in a
distinct terminal status, **`verify-failed`** (kept separate from `failed` so
"the worker errored" reads differently from "the worker claimed success but the
check disagreed"). The verify command, its exit code, and a bounded (~4 KB) tail
of its combined output are persisted on the task and shown in the TUI. In a
chain, a `verify-failed` step skips the rest, exactly like a failure.

Set it three ways, all interpolated with the builtin vars above:

- **Per definition** — a `verify:` key in the task's `config.yaml`:

  ```yaml
  # <workspace>/platform/tasks/pr-ready/config.yaml
  description: Flip a PR from WIP to ready-for-review.
  worktree: auto
  model: claude/claude-opus-4.8
  verify: gh pr view --json labels -q '.labels[].name' | grep -qx ready-for-review
  ```

  A definition step in a chain reads its `verify` live from the definition (it
  wins over any per-step override, matching how `model` behaves).

- **Ad-hoc** — `enqueue_task`'s `verify` argument.
- **Per chain step** — a `verify` field on any `enqueue_chain` step.

The full `config.yaml` key set alongside `verify`: `description`, `discovery`,
`cron`, `args`, `dedup`, `worktree`, `pre_run`, `post_run`, `model`, `timeout`,
`priority`.

## 3. Run the daemon

```bash
queohoh daemon              # foreground (first run writes a starter config)
queohoh launchd:install     # keep-alive via launchd (prints the bootstrap command)
queohoh status              # check it's up
queohoh reload              # after changing daemon code: rebuild + restart
                            # (refuses if tasks are running; --force overrides)
```

## 4. Claude Code integration

```bash
# MCP server (enqueue_task / list_tasks / list_task_definitions / run_task_definition):
claude mcp add queohoh -- queohoh mcp
```

A minimal reference `/qoo` skill ships in `examples/skills/qoo/` — copy it into `~/.claude/skills/` and it routes requests to the daemon purely through the MCP server above. It's intentionally simple; grow your own from it (see `examples/README.md`).

Interactive-session awareness (the scheduler won't run tasks in a worktree you're actively using) — add to `~/.claude/settings.json` hooks:

```json
{
  "hooks": {
    "SessionStart": [
      { "hooks": [{ "type": "command", "command": "queohoh heartbeat" }] }
    ],
    "UserPromptSubmit": [
      { "hooks": [{ "type": "command", "command": "queohoh heartbeat" }] }
    ]
  }
}
```

Heartbeats expire after 5 minutes; they're best-effort and never block.

## 5. Enqueue from anywhere

- In any Claude Code session: `/qoo <request>` — by default this queues a headless continuation of that session in the current worktree (close the tab; the daemon resumes it with full context). `/qoo status` shows the queue.
- Drop a well-formed task file into `~/.local/state/queohoh/tasks/` — that IS an enqueue.

## 6. TUI (the cockpit)

The cockpit is the Rust ratatui binary (`crates/qoo-tui`). It talks to the daemon over the unix socket, so build the workspace first (`pnpm -r build`) to compile the daemon's `dist/`, then launch the TUI:

```bash
pnpm -r build          # compile the daemon
mise run tui           # builds the release binary, self-heals the daemon, launches
```

`mise run tui` is the one-shot path; it rebuilds the TypeScript daemon, **restarts** it from *this* worktree's `packages/daemon/dist` (so a live daemon never keeps an older in-memory build), rebuilds `qoo-tui`, and launches. Pass `--no-daemon` for attach-only mode: it skips the daemon (and its TS build) *and* launches the TUI with `--no-heal`, so it never restarts a daemon owned by another checkout. Without that, the TUI's self-heal compares the daemon's build id to this worktree's `packages/daemon/dist` and two worktrees' TUIs restart the shared daemon back and forth. Use it in secondary worktrees where the daemon runs from the main checkout. To iterate on the TUI unoptimized, use `mise run tui:rs:dev`.

Run it in tmux tab 0 and leave it open — queue left, cron/worktrees right, `a` to add, `enter` for the live transcript, `q` to quit.
