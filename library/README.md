# Definition library

Versioned task definitions and Claude Code skills meant to be shared across projects. These are the source of truth; the daemon does not read this directory.

## Tasks

To use one, copy it into your workspace's global tasks dir:

```bash
cp -R library/tasks/squash-merge ~/workspace/queohoh/global/tasks/
```

Global definitions appear under every project in the TUI's definition picker (marked `(g)`); a project-local definition with the same name shadows the global one.

## Skills

`skills/qoo/` is a minimal reference `/qoo` skill: it routes a request to the queohoh MCP server (definition match → chain → ad-hoc enqueue) instead of doing the work inline. Copy it into your skills directory and adapt it:

```bash
cp -R library/skills/qoo ~/.claude/skills/
```

It is deliberately simple — treat it as the starting point for your own queue-everything workflow (session continuation, model routing, plan previews all layer on cleanly; see the "Make it your own" section inside the skill).
