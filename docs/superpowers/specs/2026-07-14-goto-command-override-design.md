# Workspace-level `goto` command override — design

## Problem

`goto` (`g` on WORKTREES and QUEUE) opens the selected worktree / resumes the selected task's Claude session in a fresh tmux window. The window command is hardcoded in the Rust TUI (`event.rs`):

- **Worktree goto** → `tmux new-window -c <path>` — a plain interactive shell rooted at the worktree.
- **Queue goto** → `tmux new-window -c <path> 'claude --resume <session_id>'` — a window that resumes the task's Claude session.

The operator wants to customize what happens on `goto` — e.g. run their own `init-tab` helper (open nvim, split the pane, run a side command) instead of a bare shell — configured once at the workspace level, applying to both goto paths.

## Constraints discovered

`init-tab` is a **zsh function**, not a PATH executable:

```zsh
init-tab () {
    command tmux send-keys 'nvim .' Enter
    command tmux split-window -h
    command tmux send-keys 'cs h' Enter
}
```

Two consequences drive the design:

1. A shell function only exists inside the operator's interactive zsh. queohoh cannot run it as an external program (`sh -c init-tab` fails — it is neither on PATH nor a builtin). The override must therefore be **typed into an interactive shell**, not exec'd.
2. `init-tab` operates by `send-keys` into the current tmux window and splitting it — i.e. it drives a window that already exists. So the natural mechanism mirrors `init-tab`'s own technique: create the window, then send the configured command as keystrokes into it.

## Design

### Config surface (workspace level)

A new optional key in the global `config.yaml` (the workspace-level config that already holds `workspace`, `projects`, `max_concurrent_tasks`):

```yaml
goto_command: "init-tab {cmd}"
```

- The value is **a line of shell typed into the new goto window**, not a program to exec.
- `{cmd}` is a placeholder substituted per goto path:
  - **worktree goto** → `{cmd}` = `""` (empty string)
  - **queue goto** → `{cmd}` = `claude --resume <session_id>`
- **Absent key → today's exact behavior is preserved** (no regression): worktree goto stays `tmux new-window -c <path>`; queue goto stays `tmux new-window -c <path> 'claude --resume <session>'`.
- Scope is workspace-only (global `config.yaml`). A per-project override (`vars.yaml`) is a deliberate non-goal (YAGNI) until there is a concrete need.

### Mechanism (`event.rs`)

When `goto_command` is set, both goto paths become the same three tmux invocations:

```
win=$(tmux new-window -P -F '#{window_id}' -c <path>)   # plain interactive zsh, rooted at the worktree
tmux send-keys -t "$win" -l -- '<interpolated goto_command>'   # -l = literal keys, no key-name lookup; -- guards a leading '-'
tmux send-keys -t "$win" Enter
```

- The new window runs the operator's interactive zsh, so functions/aliases (`init-tab`, `cs`, …) resolve — the same technique `init-tab` itself relies on.
- Capturing `#{window_id}` via `new-window -P -F` and targeting `send-keys -t "$win"` avoids a focus race (never assume the new window is the active one).
- `send-keys -l -- '<text>'` sends the command literally (no tmux key-name interpretation of the text); `Enter` is a separate `send-keys` call because it is a key name, not literal text.

When `goto_command` is **absent**, both paths fall back to today's single `tmux new-window` invocation verbatim.

### Threading (mirrors the existing `maxConcurrent` path)

1. `packages/core/src/config.ts` — add `goto_command` to `GlobalConfigSchema` (optional string) and `gotoCommand?: string` to the `GlobalConfig` interface; map it in `loadGlobalConfig`.
2. `packages/daemon/src/api.ts` — add `gotoCommand?: string` to `StateSnapshot` and populate it from `this.deps.config.gotoCommand` in `snapshot()`. Additive/optional field → the wire-compat invariant holds (an old daemon omits it → the TUI sees `None`).
3. `crates/qoo-tui/src/ipc/types.rs` — add `pub goto_command: Option<String>` to `StateSnapshot`; the container `rename_all = "camelCase"` maps `gotoCommand` automatically. `Option` (not `deserialize_with = nullable_default`) so an omitting daemon yields `None`.
4. `crates/qoo-tui/src/app/actions.rs` — `goto_worktree` and `goto_queue` read `self.snapshot`'s `goto_command` and carry it into the emitted `Cmd`: `Cmd::OpenTmux { path, goto_command }` and `Cmd::TmuxResume { path, session_id, goto_command }`.
5. `crates/qoo-tui/src/event.rs` — perform the `{cmd}` substitution and drive the tmux invocations.

Placing the raw template (not the substituted string) on the `Cmd` keeps `actions.rs` free of the per-path `{cmd}` logic; the substitution and tmux orchestration live together in `event.rs`, where all side effects already live.

### Pure planner + testing

Extract a pure function that returns the ordered tmux argv vectors, so `event.rs` only spawns them:

```rust
fn goto_tmux_plan(
    path: &str,
    session_id: Option<&str>,     // Some → queue goto (resume), None → worktree goto
    goto_command: Option<&str>,   // the raw template from config
) -> Vec<Vec<String>>
```

- No override: returns the single legacy `new-window` argv (with the `claude --resume` command appended when `session_id` is `Some`).
- Override: returns `[new-window -P -F …, send-keys -l …, send-keys Enter]`, with `{cmd}` substituted to `""` (worktree) or `claude --resume <id>` (queue).

Note the planner cannot emit the captured `#{window_id}` (it is a runtime value from the first invocation's stdout); the plan targets a placeholder token that `event.rs` replaces with the real window id between invocations. The planner is tested for the exact argv shape; `event.rs` owns only the tiny "run cmd 1, read window id, substitute, run cmds 2–3" glue.

Tests:

- Rust planner unit tests — the four cases {override, no-override} × {worktree, queue}, asserting exact argv (including `{cmd}` = empty vs `claude --resume <id>`, and the no-override fallbacks).
- Rust `ipc/types.rs` deserialize test — `gotoCommand` present → `Some`; absent → `None`.
- Existing `menu_flow_tests` / `app/tests.rs` assertions on `Cmd::OpenTmux { path }` and `Cmd::TmuxResume { path, session_id }` gain the new `goto_command` field.
- TS `config.ts` parse test — key present/absent. `api.ts` snapshot test — `gotoCommand` surfaces from config.

### Sharp edges (documented, not guarded)

- If the template omits `{cmd}`, **queue goto will not auto-resume** — there is nothing to substitute the resume command into. Literal substitution is the predictable contract; the `config.yaml` comment documents that `{cmd}` is required for queue-goto resume.
- The `send-keys` timing assumption (keys queue into the pane before the shell is fully interactive) is the same one `init-tab` already relies on and works in practice; no artificial `sleep` is introduced.

## Out of scope

- Per-project `goto_command` override.
- Any placeholder beyond `{cmd}` (e.g. `{path}` — the window is already rooted at `<path>` via `new-window -c`, so a literal path is redundant).
- Changing the non-tmux (status-line) inert behavior when `goto` runs outside tmux.
