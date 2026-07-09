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

## 3. Run the daemon

```bash
queohoh daemon              # foreground (first run writes a starter config)
queohoh launchd:install     # keep-alive via launchd (prints the bootstrap command)
queohoh status              # check it's up
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

- In any Claude Code session: `/qoo review PR 257 in platform`
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
