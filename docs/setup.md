# queohoh setup

## 1. Build & install

```bash
pnpm install && pnpm -r build
# expose the CLI (pick one):
pnpm -F @queohoh/daemon link --global   # or add packages/daemon/dist/cli.js to PATH
```

## 2. Configure

`~/.config/queohoh/config.yaml` (created with comments on first daemon start):

```yaml
workspace: ~/workspace/queohoh
projects:
  - name: platform
    path: ~/workspace/platform
max_concurrent_tasks: 3
archive_after_days: 7
vars:
  github_user: you
```

Task definitions live in the workspace, one directory per project:
`<workspace>/<project>/tasks/<name>/` (`config.yaml` + `prompt.md`). An
optional `<workspace>/<project>/vars.yaml` supplies per-project template vars.
For the config above, `platform`'s definitions live in
`~/workspace/queohoh/platform/tasks/<name>/`.

### Builtin vars

Prompts and `pre_run`/`post_run` hooks can reference these `{{var}}`
placeholders without declaring them as args. Any explicitly configured var
(global `vars`, project `vars.yaml`, or an arg of the same name) overrides the
builtin.

Resolved at **instantiate time** (definition → task):

- `{{project}}` — the registered project name (e.g. `platform`).
- `{{repo_path}}` — the project's primary-checkout path from `config.yaml`.

Resolved at **execution time** (in the task's actual worktree), via a second
render pass — so they work even for late-resolving refs (`pr:`, `ticket:`,
`temp`):

- `{{worktree}}` — the resolved worktree/lane name.
- `{{worktree_path}}` — its absolute path.
- `{{branch}}` — `git rev-parse --abbrev-ref HEAD` in that worktree (empty if
  it can't be read).
- `{{ticket}}` — the ticket id derived from the branch name (convention: the
  branch is named after its ticket, so `jus-1008-fix-thing` → `JUS-1008`;
  empty when the branch has no ticket-shaped token).

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

# /qoo skill:
ln -s "$(pwd)/skills/qoo" ~/.claude/skills/qoo
```

Interactive-session awareness (the scheduler won't run tasks in a worktree
you're actively using) — add to `~/.claude/settings.json` hooks:

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

- In any Claude Code session: `/qoo <request>` — by default this queues a
  headless continuation of that session in the current worktree (close the
  tab; the daemon resumes it with full context). `/qoo status` shows the
  queue.
- Drop a well-formed task file into `~/.local/state/queohoh/tasks/` — that IS an enqueue.

## 6. TUI (the cockpit)

From a fresh checkout, build the workspace first (`pnpm -r build`) so the TUI's
`@queohoh/core` / `@queohoh/daemon` dependencies resolve to compiled `dist/`.

```bash
pnpm -r build                     # or at least: pnpm -F @queohoh/tui build
node packages/tui/dist/cli.js     # or `queohoh-tui` after pnpm link --global
```

Run it in tmux tab 0 and leave it open — queue left, cron/worktrees right,
`a` to add, `enter` for the live transcript, `q` to quit.
