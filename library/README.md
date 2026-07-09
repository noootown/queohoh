# Definition library

Versioned task definitions meant to be shared across projects. These are the
source of truth; the daemon does not read this directory.

To use one, copy it into your workspace's global tasks dir:

```bash
cp -R library/tasks/squash-merge ~/workspace/queohoh/global/tasks/
```

Global definitions appear under every project in the TUI's definition picker
(marked `(g)`); a project-local definition with the same name shadows the
global one.
