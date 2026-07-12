# Worktree Menu Removal + Session Picker Design

Date: 2026-07-11
Status: Approved

## Goal

Kill the worktrees-pane action menu. Its four rows become: two pane hotkeys (`g` goto tmux, `x` remove) and one combined new-task flow (`r`), which merges "New task (fresh session)" and "New task (main session)" into a single session-picker → multiline-prompt flow. The "main session" concept is removed entirely, replaced by a session lineage map that keeps chained tasks stacking correctly while allowing the user to pin any recent session.

## Background

Today the worktree action menu (`action_menu.rs::worktree_menu`) holds: New task (fresh session), New task (main session), Open in tmux window, Remove worktree. The fresh/main split forces the user to understand the daemon's per-lane `MainSessionStore` pointer. Meanwhile Claude Code already auto-writes human-readable session titles (`{"type":"ai-title", ...}` records) into every session jsonl under `~/.claude/projects/<encoded-path>/`, so recent sessions can be listed by name with no rename convention or CLAUDE.md instruction.

## Section 1 — UX flow & keymap

### Worktrees pane (action menu deleted)

- `r` **[r]un** — opens the new-task flow on the selected worktree row. Replaces both "New task" menu rows. `r` is currently inert on the worktrees pane (Run chip exists only on QUEUE/TASKS), so no conflict, and it matches the existing "r = run something here" convention.
- `g` **[g]oto** — open the worktree in a new tmux window (`MenuAction::OpenTmux` behavior). Also works on interactive-session rows in the same pane, so their one-row menu dies too. Disabled (status-line no-op) outside tmux.
- `x` **[x] remove** — remove worktree + delete local branch, routing through the existing confirm modal. No-op with a status-line message when a task is running there (`WtState::Busy`).
- `t` (task menu), `c` (create), `z` (collapse), `/` (search) unchanged.
- The `a` (actions) chip disappears from the worktrees pane. `worktree_menu()` and its `MenuAction::TaskFresh`/`TaskMain` variants are deleted.
- The queue pane's single-row Resume menu is untouched — a follow-up step in the broader menu-removal campaign.

### Keymap cleanup

- `g`/`G` jump-to-top/bottom (`AppAction::ScrollEdge`) deleted globally. `G` becomes unbound; `g` is a worktrees-pane-gated goto chip.
- `q` stays Quit (no conflict since the new-task verb is `r`).

### New-task flow (two steps, Esc cancels either)

1. **Session picker** — a list modal in the same visual style as the current action menu (title = worktree name, type-to-filter, description in the right pane). Rows: first **"New session"**, then the last 5 sessions for the worktree, each labeled `<human name> · <relative age>` (e.g. `Redesign TUI to full page layout · 2h ago`). Worktree with no sessions shows only "New session".
2. **Prompt editor** — multiline input. **Enter submits, Shift+Enter inserts a newline.** All alt+enter bindings are removed app-wide: the `def_args.rs` newline arm `Enter if shift || alt` becomes shift-only. The `Mode::AddTask` single-line `tui_input::Input` is replaced with the same multiline field widget the DefArgs form already uses (`insert_newline()` machinery) — first step of app-wide text-input unification.

`SessionMode::Fresh/Main` disappears from the TUI. The enqueue carries either no session pin (fresh) or `resume_session_id` (picked session).

## Section 2 — Session listing, labeling & backend

### Session discovery (daemon-side)

New IPC method `list_sessions { worktree }`, requested on demand when the picker opens (no snapshot bloat; the TUI already has a generic `call(method, params)` RPC). The daemon:

1. Encodes the worktree's absolute path the way Claude Code does (`/` and `.` → `-`) and reads `~/.claude/projects/<encoded>/*.jsonl` — top-level files only (subdirectories hold subagent transcripts and are skipped).
2. Sorts by file mtime descending, takes the top 5.
3. Labels each session with the first hit in this chain:
   - **queohoh task summary** — if the run store maps this sessionId to a run, use that task's summary (best label for worker-spawned sessions);
   - **last `ai-title` record** in the jsonl — Claude's auto-generated title (covers interactive sessions);
   - **first user prompt line**, truncated;
   - short session-id prefix as last resort.
4. Returns `{ sessionId, label, mtime }[]`.

No CLAUDE.md rename convention is needed anywhere.

### Chain correctness — `SessionLineageStore` replaces `MainSessionStore`

Mechanics today: headless `claude -p --resume X` mints a new session id Y for the run; the worker advances a per-lane pointer to Y so a queued task pinned to X (created before Y existed) resumes Y instead — that's what makes chains stack. But the pointer is per-lane, not per-chain: with a free picker, task A pinned to session #1 finishing would hijack task B deliberately pinned to older session #3 in the same lane.

Replacement:

- New `SessionLineageStore` (same atomic JSON-file pattern as `MainSessionStore`): when a run resuming session X produces session Y, record the fork `X → Y`.
- At spawn, a pinned task follows its pin's lineage to the tip (X → Y → Z ⇒ resume Z). Cycle-guarded; a missing link stops at the last known hop.
- Fresh tasks record nothing to follow — their resulting session becomes a lineage root for future picks.
- `MainSessionStore`, the lane pointer, the `ptr.updatedAt > task.created` redirect, and `task.session: "main"` semantics are deleted. The `session` field stays accepted on the HTTP/MCP surface for back-compat: `"main"` logs a deprecation warning and is treated as fresh.

MCP tools already pass `resume_session_id`, so `qoo`-skill chains keep working — two chained tasks pinned to the same session stack exactly as before, now via lineage instead of the lane pointer.

### Edge cases

- Picking a session a worker is currently running on: allowed — per-worktree lane serialization already queues the task, and lineage resolution at spawn time picks up the tip once the running task finishes.
- Worktree with no sessions yet: picker shows only "New session".
- Short/headless sessions with no `ai-title`: fallback chain covers them.

## Testing

- `action_menu.rs`: delete `worktree_menu` builder tests; session-row menu tests replaced by goto-hotkey behavior tests.
- `keymap.rs`: `r`/`g`/`x` gated to the worktrees pane; `g`/`G` ScrollEdge arms removed; `q` still quits.
- Session picker: filter/selection tests mirroring existing menu-flow tests (`menu_flow_tests.rs` pattern).
- Prompt editor: Enter submits, Shift+Enter inserts newline, alt+enter inert (def_args + new AddTask editor).
- Daemon `list_sessions`: encoding, top-5 mtime ordering, label fallback chain (fixture jsonl files), subdirectory exclusion.
- `SessionLineageStore`: fork recording, tip resolution across multi-hop chains, cycle guard, two-chains-in-one-lane isolation (the hazard the lane pointer had).
- Worker: pinned task resumes lineage tip; fresh task records root; `session:"main"` back-compat maps to fresh.
