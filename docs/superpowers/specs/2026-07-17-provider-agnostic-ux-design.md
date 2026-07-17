# Provider-Agnostic UX — Design

**Date:** 2026-07-17
**Status:** Approved (design), pending implementation plan
**Builds on:** `docs/superpowers/specs/2026-07-16-model-catalog-provider-switch-design.md` (catalog, `resolveModelChain`, `active_provider` re-head — already implemented)

## Motivation

The catalog + provider switch made multi-provider runs *possible*, but several operator surfaces still assume Claude-only or show the authored model without reflecting the active provider:

1. TASKS Model column shows the authored `provider/label` (or list), not the effective head under the current switch.
2. Session picker does not show which provider owns a session.
3. Definition config tab does not emphasize the full model chain.
4. Worktree/queue `goto` still routes through `goto_command: 'init-tab "{cmd}"'` (nvim left, agent right) and hardcodes `claude --resume` for queue resume.
5. Run-from-def model picker lists the whole catalog with a vague "default" head, not the def's effective chain under the switch.
6. Most workspace defs still pin a single Claude model, so re-head has little authored backup material.
7. Discovery `d` has no confirm and is easy to fat-finger into a fan-out.

This design is a thin UX + config layer on the existing chain machinery — not a new resolution model.

## Decisions log (from design discussion)

- Display and run-picker use the **effective chain** from `resolveModelChain` (re-head + group-head prepend). Authored chain stays the source of truth in yaml and in the config detail tab.
- Every workspace def gets **multi-provider backup lists** paired by judgment tier (opus/fable → grok-4.5; sonnet → grok-4.5 until a real sonnet-class Grok model exists). No silent bare-tier aliases.
- **No** `providers.grok.system_prompt` / queohoh `--rules` injection for model routing.
- Grok-native subagent model router lives in **`~/dotfiles/grok/AGENTS.md`**, symlinked to `~/.grok/AGENTS.md` (same control pattern as `~/dotfiles/claude-code/CLAUDE.md` → `~/.claude/CLAUDE.md`).
- Goto: **first-class TUI layout** — new tmux window, left|right split, left bare shell, right = `cmd`. Kill `init-tab` and the `goto_command` config key. Still pass a real `cmd` (fresh agent or provider-aware resume).
- Worktree goto: **provider picker** then fresh interactive agent. Queue goto: **no picker** — resume with the task's recorded provider + session.
- Cron already re-heads via `active_provider` at run start (same worker path as manual runs); no cron-specific provider. Document only.
- Discovery `d`: existing Confirm modal before RPC.

## 1. Effective model display (TASKS + config)

### Semantics (unchanged)

`resolveModelChain(def.model, catalog, providers, default_models, active_provider)` remains the authority:

1. Authored list, or `default_models` when `model` is absent.
2. Drop disabled providers.
3. Stable-partition: active-provider entries first.
4. If none for active provider → prepend that provider's catalog group head.
5. Dedup → effective chain.

Cron, TUI run, MCP, and ad-hoc all share this path. Cron does **not** freeze provider at fire time: `fireCron` only enqueues; `buildWorkerDeps` reads `activeProvider()` when the worker starts.

### TASKS pane Model column

Show **only the effective head** as `provider/label` (e.g. `grok/grok-4.5` when switched to grok and the chain re-heads there).

- Recompute when `active_provider` or settings/catalog changes (TUI already has both on the snapshot/settings payload).
- Empty/missing when resolution would fail (no runnable model) — prefer a dim `—` over a stale authored ref.
- Column width rules unchanged (fixed width, pane-gated); do not size from full-chain text.

### Definition config tab (detail)

`model` row shows the **full authored chain** joined with ` → `:

```
model      claude/opus → grok/grok-4.5
```

Absent `model:` → em dash (resolves via `default_models` at run time), same as today.

No second "effective" line in v1 — the TASKS column is the live effective view.

## 2. Definition model lists — tiered backups

### Authoring convention (judgment tiers)

Cross-provider "same tier" is **not** restored as automatic resolution. Tiers exist only as an **authoring convention** when writing multi-entry lists:

| Judgment band | Claude (primary) | Grok backup |
|---|---|---|
| judgment (open-ended) | `claude/opus` or `claude/fable` | `grok/grok-4.5` |
| mechanical (resolved + checkable) | `claude/sonnet` | `grok/grok-4.5` for now |

- **Today:** `grok models` only lists `grok-4.5`. Catalog may still list `composer`; do **not** migrate sonnet defs to `grok/composer` until that model is actually runnable. When it is, mechanical backups flip to `grok/composer` in a one-line follow-up.
- **haiku:** still banned for authoring (matches `CLAUDE.md`); do not add haiku backups.
- Single-entry lists remain valid for rare "exact this model only" intent, but the workspace migration makes multi-provider the default posture.

### Migration (config workspace)

Every def under `~/workspace/queohoh/*/tasks/*/config.yaml` with a single Claude ref becomes a two-entry list:

```yaml
# was: model: claude/opus
model: [claude/opus, grok/grok-4.5]

# was: model: claude/sonnet
model: [claude/sonnet, grok/grok-4.5]
```

Already multi-provider lists: leave as-is if they already include a grok entry; otherwise append the tier-appropriate grok backup.

`default_models` in `config.yaml` stays a fallback for model-less tasks; after migration most defs carry their own list.

## 3. Run picker (definition `r`)

When launching a definition from the TASKS pane:

- Options = **effective chain** for that def under current `active_provider` (not the full catalog).
- Labels: `label (provider)`; values: `provider/label`.
- **Preselect** chain\[0\] (already active-provider-headed).
- Submitting a concrete model = 1-entry list (exact, no rotation) — same contract as today's non-default catalog pick.
- **No** empty "default (…)" head for def launch: preselect and submit the chosen ref. (Active provider changing while the form is open: user re-picks or cancels/reopens.)

Ad-hoc create (`c`) keeps the broader catalog dropdown (no def chain to constrain). Resume / session-pin flows keep preferred-model preselect validated against catalog options where those forms still use the catalog field.

## 4. Session picker — show provider

### Data

`listSessions` already maps run-stored model ids back to `provider/label`. Extend each session entry with an explicit **`provider`** string when known:

1. Provider segment of the mapped `provider/label` model ref, else
2. `SessionLineageStore.providerOf(sessionId)`, else
3. Omit (`null` / absent) — never guess.

Wire: optional `provider` on the session list payload; TUI `SessionChoice` gains `provider: Option<String>` with `serde(default)`.

### UI

Session rows keep label + right-floated age. Add a dim provider tag before the age, e.g.:

```
# PR Resolve Comments              claude  1h ago
```

No new column layout for v1. Rows without a known provider look as they do today (no tag).

### Out of scope

Discovering pure Grok filesystem sessions that never went through queohoh. Only sessions already listed (Claude project dir + run-linked models/lineage) get a tag.

## 5. Goto — first-class tmux tab + split

### Remove

- Workspace `goto_command` / `init-tab` usage (`config.yaml` and schema field).
- Hardcoded `claude --resume` assumption in queue goto.

Dotfiles may keep `init-tab` for manual shell use; queohoh must not call it.

### Built-in layout (TUI `event.rs` / `GotoPlan`)

Always:

1. `tmux new-window -c <path>` (capture window id if needed for targeting).
2. Split **left | right** (`split-window -h -c <path>` on the new window).
3. **Left pane:** bare interactive shell — no `send-keys`, no nvim.
4. **Right pane:** run **`cmd`** (send-keys + Enter, or equivalent).

`cmd` is always produced by the TUI from provider + mode — not a user shell template.

### Worktree pane `g`

1. Require inside tmux + a selected worktree (same gates as today).
2. Open a small **provider picker** (enabled providers only, catalog/settings order).
3. On pick: `cmd = <interactive bin for provider>` with no resume (fresh session).
   - Interactive bins: from effective provider config `bin` when set, else the provider name / adapter default (`claude`, `grok`, …). Fresh interactive = bare bin (no `-p` / headless flags). Example: config `providers.grok.bin: /Users/…/.local/bin/grok` must be what the right pane runs, not a bare `grok` that misses PATH.
4. Execute built-in layout with that `cmd`.

### Queue pane `g`

1. Same gates as today (tmux, session id, worktree path).
2. **No provider menu.** Resolve provider from the task's run / lineage (session's provider tag; fall back to model ref's provider; last resort `claude` only for untagged legacy).
3. `cmd = <bin> --resume <session_id>` (same bin resolution as worktree; claude and grok both use `--resume` today; if a future adapter differs, map via adapter metadata rather than hardcoding claude).
4. Execute built-in layout.

### Config / API

- Delete `goto_command` from global config schema, daemon snapshot field, and tests that seed `init-tab {cmd}`.
- Remove CreateAndSend-for-goto_command path once the built-in split plan replaces it (or repurpose a single internal plan type for "new window + split + right cmd").
- Settings / snapshot must expose enough for the TUI to build `cmd`: at minimum each enabled provider's **`name` + effective `bin`** (today settings only ships name/enabled — extend with optional `bin` so grok's pinned path is not lost).

## 6. Grok AGENTS.md (dotfiles, not queohoh daemon)

### Placement (mirror Claude)

| Role | Path |
|---|---|
| Source of truth | `~/dotfiles/grok/AGENTS.md` |
| Runtime | `~/.grok/AGENTS.md` → symlink to the source |

Same idea as `~/dotfiles/claude-code/CLAUDE.md` → `~/.claude/CLAUDE.md`.

### Content

Grok-native subagent model router (judgment-based, not task-type-based), parallel to the Claude block in `CLAUDE.md`:

- **grok-4.5** — judgment not yet resolved (explore, unknown debug, review). Default when open-ended.
- **composer** — judgment resolved + checkable (mechanical edits, tests, known fixes). If composer is unavailable, use grok-4.5.
- Explicit `model:` on every spawn; no silent inherit-and-downgrade.

Grok loads `AGENTS.md` via project-rules discovery (including `~/.grok/`). **No** `providers.grok.system_prompt` in queohoh config.

If dotfiles install scripts currently wire Claude symlinks, add the Grok symlink the same way (or document a one-line `ln -sf` in the install notes).

## 7. Discovery confirm

`d` / Discover chip on TASKS:

1. Open existing `Mode::Confirm` (same pattern as provider-switch / remove worktree).
2. Body: `Run discovery for {repo}/{name}?` (optional second line: can fan out many tasks).
3. Confirm → existing `discover_selected_def` / `discoverDefinition` RPC.
4. Cancel → no-op, no RPC.

No daemon change.

## 8. Testing

- **TUI selectors:** effective head for TASKS model column under active_provider re-head and group-head prepend; authored chain display in config tab.
- **TUI forms:** def-run model options = effective chain; preselect chain\[0\]; submitting one ref.
- **TUI session pick:** provider tag present/absent; snapshot/layout tests.
- **TUI/event goto:** plan builds new-window + split + right cmd; worktree includes provider; queue uses resume cmd with non-claude provider; no `goto_command` / init-tab.
- **TUI actions:** discover opens Confirm first; confirm fires RPC; cancel does not.
- **Core/daemon:** no resolution algorithm change required; existing `resolveModelChain` + cron path tests remain the proof that cron honors `active_provider` at run time. Add a short regression comment or test name if useful so this does not regress silently.
- **Config migration:** inventory script or checklist that every migrated def has ≥2 provider segments in `model:`.

## 9. Out of scope

- Multi-source Grok session filesystem discovery.
- Resurrecting bare-tier aliases (`model: opus`) or automatic cross-provider equivalence tables.
- codex enablement.
- TUI widget for free-form multi-model list editing.
- Injecting model-router text via `providers.*.system_prompt`.
- Changing resume/lineage pin semantics or availability-classification regexes.
- Replacing `init-tab` in dotfiles (queohoh simply stops calling it).

## Implementation sketch (for the plan, not binding)

| Area | Likely touch points |
|---|---|
| TUI effective head | `selectors.rs` (`def_model_text`), needs catalog + active_provider + default_models inputs |
| Config tab chain | `view/detail.rs` (already shows list; ensure Many path is the default story) |
| Def run model field | `app/form.rs` / `def_args.rs` — chain-based options for def launch |
| Sessions | daemon `listSessions`, `ipc/types.rs`, `view/menu.rs` session pick |
| Goto | `event.rs` GotoPlan, `actions.rs` goto_worktree/goto_queue, keymap provider pick mode, delete goto_command from core/config + types |
| Discovery confirm | `app/actions.rs` + `ConfirmAction` variant |
| Def migration | `~/workspace/queohoh/**/tasks/**/config.yaml` |
| Grok rules | `~/dotfiles/grok/AGENTS.md` + symlink `~/.grok/AGENTS.md` |
| Config cleanup | remove `goto_command` from `~/workspace/queohoh/config.yaml` |
