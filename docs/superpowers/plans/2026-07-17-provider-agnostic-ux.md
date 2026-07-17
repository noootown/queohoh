# Provider-Agnostic UX Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make multi-provider operation visible and operable in the TUI: effective-head model display, def run chain picker, session provider tags, first-class tmux goto (kill init-tab), discovery confirm, tiered def backups, and dotfiles-managed Grok AGENTS.md.

**Architecture:** Thin layer on the existing catalog + `resolveModelChain` + `active_provider` stack. Daemon grows small wire fields (`providers[].bin`, session `provider`). TUI ports chain resolution for pure display/pickers, rewrites `GotoPlan` to always new-window + left|right split, and adds Confirm before discovery. Config/dotfiles migrations live outside the daemon binary.

**Tech Stack:** TypeScript (`packages/core`, `packages/daemon` — vitest), Rust (`crates/qoo-tui` — ratatui, cargo test), workspace yaml under `~/workspace/queohoh`, dotfiles under `~/dotfiles/grok`.

**Spec:** `docs/superpowers/specs/2026-07-17-provider-agnostic-ux-design.md` — read it before starting any task.

## Global Constraints

- Chain semantics stay in `resolveModelChain` (TS) / a Rust mirror for TUI: re-head + group-head prepend; no bare-tier aliases.
- Authored def lists stay multi-provider after migration (`[claude/…, grok/grok-4.5]`); TASKS column shows **effective head only**.
- Def run picker options = **effective chain** for that def; preselect chain\[0\]; submit 1-entry exact model.
- Goto: always new tmux window, left bare shell | right `cmd`; no `goto_command`, no `init-tab`. Worktree picks provider (fresh bin); queue resumes that task’s provider.
- Interactive/resume `cmd` uses settings `providers[].bin` when set, else provider name.
- Grok router text: `~/dotfiles/grok/AGENTS.md` → symlink `~/.grok/AGENTS.md`. No `providers.grok.system_prompt`.
- Wire compat: new fields optional/`serde(default)` so old daemons still deserialize.
- Repo conventions: no Co-Authored-By; commit per task with explicit paths; `pnpm --filter @queohoh/core test` / `pnpm --filter @queohoh/daemon test` / `cargo test -p qoo-tui`.

## File Structure

```
packages/core/src/config.ts              remove goto_command schema + GlobalConfig field
packages/daemon/src/api.ts               settings providers + bin; listSessions.provider; drop gotoCommand from snapshot
packages/daemon/src/__tests__/api.test.ts  wire tests
crates/qoo-tui/src/chain.rs              (new) pure resolve_model_chain mirror of packages/core models.ts
crates/qoo-tui/src/selectors.rs          def_model_text → effective head
crates/qoo-tui/src/ipc/types.rs          SettingsProvider.bin; drop goto_command; session types
crates/qoo-tui/src/event.rs              SessionChoice.provider; GotoPlan rewrite; Cmd shapes
crates/qoo-tui/src/app/form.rs           def-launch model dropdown from effective chain
crates/qoo-tui/src/app/def_args.rs       wire model field for def run
crates/qoo-tui/src/app/actions.rs        goto provider pick; queue provider-aware resume; discovery confirm
crates/qoo-tui/src/app/mode.rs           ConfirmAction::DiscoverDef; provider-pick mode if needed
crates/qoo-tui/src/view/menu.rs          session provider tag
crates/qoo-tui/src/view/detail.rs        authored chain already OK — verify only
~/workspace/queohoh/**/tasks/**/config.yaml  model list migration
~/workspace/queohoh/config.yaml          remove goto_command
~/dotfiles/grok/AGENTS.md                new; symlink ~/.grok/AGENTS.md
```

---

### Task 1: Daemon — settings `bin` + session `provider`; drop `gotoCommand`

**Files:**
- Modify: `packages/core/src/config.ts` (remove `goto_command` from schema and `GlobalConfig` / `loadGlobalConfig`)
- Modify: `packages/daemon/src/api.ts` (settings map; listSessions; snapshot)
- Test: `packages/core/src/__tests__/config.test.ts`, `packages/daemon/src/__tests__/api.test.ts`

**Interfaces:**
- Produces on settings RPC `providers[]`: `{ name: string, enabled: boolean, bin?: string }` (`bin` omitted when unset).
- Produces on `listSessions` session objects: existing fields + optional `provider: string` when known (from model ref’s provider segment, else lineage `providerOf(sessionId)`).
- Removes: `gotoCommand` from state snapshot / `ApiServer` snapshot builder; `goto_command` from config parse.

- [ ] **Step 1: Failing tests**

```ts
// api.test.ts — settings includes bin when configured
it("settings providers include optional bin", async () => {
  const { client } = await setup({
    /* seed config providers with grok bin: /tmp/grok-bin */
  });
  const s = await client.call("settings");
  const grok = s.providers.find((p) => p.name === "grok");
  expect(grok).toMatchObject({ name: "grok", enabled: true, bin: "/tmp/grok-bin" });
  const claude = s.providers.find((p) => p.name === "claude");
  expect(claude.bin).toBeUndefined();
});

// listSessions includes provider
it("listSessions includes provider from model mapping or lineage", async () => {
  // existing listSessions fixture that already maps model → claude/opus:
  // expect sessions[0].provider === "claude"
});

// config.test.ts — goto_command no longer parsed (or ignored with no field)
it("does not surface gotoCommand", () => {
  const cfg = loadGlobalConfig(/* yaml with goto_command still present */);
  expect(cfg.gotoCommand).toBeUndefined();
});
```

- [ ] **Step 2: Run** `pnpm --filter @queohoh/daemon test -- api` and `pnpm --filter @queohoh/core test -- config` → FAIL on new assertions.

- [ ] **Step 3: Implement**
  - `api.ts` settings: `providers: deps.config.providers.map((p) => ({ name: p.name, enabled: p.enabled, ...(p.bin ? { bin: p.bin } : {}) }))`
  - `listSessions`: for each session, `provider` from `aliasForModel(...)?.split("/")[0]` or `deps.lineage.providerOf(sessionId)` when non-null
  - Delete `gotoCommand` from snapshot type and `state` payload construction in `api.ts`
  - `config.ts`: remove `goto_command` from zod schema and `gotoCommand` from `GlobalConfig` / load mapping; update `config.test.ts` expectations (legacy yaml key may still parse if left in schema as ignored — prefer **delete** so it is not reintroduced)

- [ ] **Step 4:** daemon + core suites green.

- [ ] **Step 5: Commit** `feat(daemon,core): settings bin + session provider; remove goto_command`

---

### Task 2: TUI wire types — `bin`, session `provider`, drop `goto_command`

**Files:**
- Modify: `crates/qoo-tui/src/ipc/types.rs`
- Modify: `crates/qoo-tui/src/event.rs` (`SessionChoice`)
- Modify: `crates/qoo-tui/src/test_fixtures.rs`
- Test: unit tests in `types.rs` / `event.rs` as existing style

**Interfaces:**
- `SettingsProvider { name, enabled, bin: Option<String> }` with `#[serde(default)]` on `bin`
- `SessionChoice { …, provider: Option<String> }` with `#[serde(default)]`
- `StateSnapshot.goto_command` **removed** (or kept `#[serde(default)]` ignored — prefer remove + fix all fixtures)

- [ ] **Step 1: Failing tests** for deserializing settings with `bin` and sessions with `provider`; snapshot without `gotoCommand` still works.

- [ ] **Step 2: Implement types + fixture updates.**

- [ ] **Step 3:** `cargo test -p qoo-tui` green for type/fixture tests.

- [ ] **Step 4: Commit** `feat(tui): wire bin + session provider; drop goto_command field`

---

### Task 3: TUI pure `resolve_model_chain` + TASKS effective head

**Files:**
- Create: `crates/qoo-tui/src/chain.rs` (or `selectors` submodule — prefer dedicated file mirrored on `packages/core/src/models.ts`)
- Modify: `crates/qoo-tui/src/lib.rs` / `main` module tree to `mod chain;`
- Modify: `crates/qoo-tui/src/selectors.rs` — `def_model_text` takes resolution context or a precomputed head
- Modify: call sites that build def rows / layout (`selectors` + view if needed) to pass catalog, enabled providers, default_models, active_provider
- Test: `chain.rs` unit tests; update `def_model_text` tests in `selectors.rs`

**Interfaces:**

```rust
// crates/qoo-tui/src/chain.rs
pub struct ChainEntry { pub provider: String, pub model_id: String, pub model_ref: String }

/// Mirrors packages/core resolveModelChain exactly:
/// refs from spec or default_models → find in catalog → drop disabled →
/// active first → prepend group head if miss → dedup by provider/id.
pub fn resolve_model_chain(
    spec: Option<&ModelRef>,           // None = use defaults
    catalog: &[CatalogEntry],
    enabled_providers: &[&str],        // or &[SettingsProvider]
    default_models: &[String],
    active_provider: &str,
) -> Result<Vec<ChainEntry>, String>;

pub fn effective_model_head(/* same args */) -> Option<String>; // Ok chain[0].model_ref
```

`def_model_text(def, ctx) -> String` returns `effective_model_head(...).unwrap_or_default()` (or `"—"` only if the view needs a dash — match existing empty-string pane-gate behavior unless tests require dash).

- [ ] **Step 1: Failing chain tests** (port key cases from `packages/core/src/__tests__/models.test.ts`):
  - `[claude/opus, grok/grok-4.5]` + active grok → head `grok/grok-4.5`
  - `[claude/opus]` + active grok → head `grok/grok-4.5` (group-head prepend)
  - null spec uses default_models then re-heads
  - disabled provider entries dropped

- [ ] **Step 2: Implement `chain.rs`.**

- [ ] **Step 3: Failing `def_model_text` test** — def authored `claude/opus`, active grok, catalog with both groups → displays `grok/grok-4.5` not `claude/opus`.

- [ ] **Step 4: Wire selectors/view** so layout and render use the same effective head (pass settings snapshot fields available on `App` / `Computed`).

- [ ] **Step 5:** `cargo test -p qoo-tui` green.

- [ ] **Step 6: Commit** `feat(tui): effective-head model column via resolve_model_chain`

---

### Task 4: Def run model picker = effective chain

**Files:**
- Modify: `crates/qoo-tui/src/app/form.rs`
- Modify: `crates/qoo-tui/src/app/def_args.rs` (where def run form builds fields)
- Test: `crates/qoo-tui/src/app/form_tests.rs`

**Interfaces:**
- New helper e.g. `fn def_model_field(&self, repo: &str, def_model: Option<&ModelRef>) -> Field`
  - options = `resolve_model_chain(def_model, …)` mapped to `DropdownOption { value: model_ref, label: label (provider) }` (resolve label via catalog)
  - **no** empty `""` head option
  - default value = first option’s `model_ref`
- Ad-hoc `model_field` **unchanged** (full catalog + default head).
- Def run submit continues to send the selected model string as today (1-entry exact).

- [ ] **Step 1: Failing tests**
  - Def `model: [claude/opus, grok/grok-4.5]`, active_provider=grok → options exactly those two refs in effective order (grok first), preselect `grok/grok-4.5`
  - Def `model: claude/opus` only, active=grok → options include prepended `grok/grok-4.5` then `claude/opus` (mirror chain)
  - Ad-hoc create still has `default (…)` head + catalog entries

- [ ] **Step 2: Implement + wire `open_def_args` / zero-arg run path** so every def launch uses `def_model_field`.

- [ ] **Step 3:** `cargo test -p qoo-tui` green.

- [ ] **Step 4: Commit** `feat(tui): def run model picker uses effective chain`

---

### Task 5: Session picker shows provider

**Files:**
- Modify: `crates/qoo-tui/src/view/menu.rs` (`render_session_pick`)
- Modify: tests in `menu.rs` / session pick tests
- Daemon already from Task 1

**Interfaces:**
- Row layout: label left; dim `provider` (when `Some`) then age right-floated.
- Example: `# PR Resolve Comments              claude  1h ago`

- [ ] **Step 1: Failing render test** — item with `provider: Some("claude")` contains `claude` near the age; item with `None` has no stray provider token.

- [ ] **Step 2: Implement render.**

- [ ] **Step 3:** `cargo test -p qoo-tui` green.

- [ ] **Step 4: Commit** `feat(tui): show provider tag on session picker rows`

---

### Task 6: Goto — first-class split; provider pick; kill init-tab

**Files:**
- Modify: `crates/qoo-tui/src/event.rs` (`GotoPlan`, `goto_tmux_plan` → new builder, `Cmd::OpenTmux` / `Cmd::TmuxResume` or unified `Cmd::Goto { path, cmd }`)
- Modify: `crates/qoo-tui/src/app/actions.rs` (`goto_worktree`, `goto_queue`)
- Modify: `crates/qoo-tui/src/app/mode.rs` (provider pick mode or reuse menu/confirm pattern)
- Modify: keymap/mouse if a new mode needs keys
- Test: `event.rs` plan unit tests; `app/menu_flow_tests.rs` / actions tests that currently assert `init-tab`

**Interfaces:**

```rust
// Preferred Cmd shape (simplify):
Cmd::Goto {
  path: String,
  /// Right-pane command. Empty string = right pane is also a bare shell (rare).
  cmd: String,
}

pub(crate) enum GotoPlan {
  /// new-window -P -F #{window_id} -c path;
  /// split-window -h -t id -c path;
  /// if !cmd.is_empty(): select-pane right; send-keys -l cmd; send-keys Enter
  Split { path: String, cmd: String },
}

pub(crate) fn goto_split_plan(path: &str, cmd: &str) -> GotoPlan;
```

**Worktree `g`:**
1. Existing tmux/selection gates.
2. Open provider picker: enabled providers from settings (`name` list). UI: small menu/modal (follow existing `Mode` patterns — e.g. simple list like menu items, or reuse DefPick-style chrome if lighter).
3. On pick provider `p`: `bin = settings.providers.find(p).bin.as_deref().unwrap_or(p.name)`; `cmd = bin.to_string()` (fresh).
4. `Cmd::Goto { path, cmd }`.

**Queue `g`:**
1. Resolve `session_id` + `path` as today.
2. Resolve provider: run meta / task model provider segment / lineage if available on snapshot; else `"claude"` for legacy untagged.
3. `bin` as above; `cmd = format!("{bin} --resume {session_id}")`.
4. `Cmd::Goto { path, cmd }` — **no** provider menu.

Delete all `goto_command` threading from actions, fixtures, and tests.

- [ ] **Step 1: Failing plan tests**

```rust
// new-window + split-h + send-keys for non-empty cmd
// empty cmd: new-window + split, no send-keys on right (or both shells)
// no CreateAndSend / init-tab variants remain
```

- [ ] **Step 2: Implement `GotoPlan` + `run_goto` tmux sequence** (target the new window’s panes carefully: capture `window_id`, split `-t window_id`, send-keys to the right pane).

- [ ] **Step 3: Failing action tests** — worktree goto emits provider pick mode (or Cmd after pick); queue goto emits `Goto` with `grok --resume …` when provider is grok and bin default; no `init-tab` strings.

- [ ] **Step 4: Implement actions + provider pick mode + confirm path.**

- [ ] **Step 5:** `cargo test -p qoo-tui` green.

- [ ] **Step 6: Commit** `feat(tui): first-class goto split; provider pick; remove init-tab path`

---

### Task 7: Discovery confirm dialog

**Files:**
- Modify: `crates/qoo-tui/src/app/mode.rs` — `ConfirmAction::DiscoverDef { repo, name }`
- Modify: `crates/qoo-tui/src/app/actions.rs` — `discover_selected_def` opens Confirm instead of RPC
- Modify: `crates/qoo-tui/src/app/update.rs` — `run_confirm_action` arm
- Test: actions / update tests

**Interfaces:**
- Body: `Run discovery for {repo}/{name}?` (optional second line about fan-out).
- Confirm → existing discovering insert + `discover_definition_cmd`.
- Cancel → no RPC, no discovering insert.

- [ ] **Step 1: Failing test** — `DiscoverSelectedDef` leaves mode `Confirm { action: DiscoverDef {..} }` and **no** `Cmd::Rpc` yet; confirm then produces discover RPC.

- [ ] **Step 2: Implement.**

- [ ] **Step 3:** `cargo test -p qoo-tui` green.

- [ ] **Step 4: Commit** `feat(tui): confirm before definition discovery`

---

### Task 8: Workspace def migration + config.yaml cleanup

**Files (outside this git repo — commit in their own repos):**
- `~/workspace/queohoh/*/tasks/*/config.yaml` — every single-model Claude def → two-entry list
- `~/workspace/queohoh/config.yaml` — remove `goto_command` block and its comments

**Mapping:**

| Authored | Migrated |
|---|---|
| `claude/opus` | `[claude/opus, grok/grok-4.5]` |
| `claude/sonnet` | `[claude/sonnet, grok/grok-4.5]` |
| `claude/fable` | `[claude/fable, grok/grok-4.5]` |
| already multi + has grok | leave |
| already multi without grok | append tier-appropriate grok backup |

- [ ] **Step 1: Inventory**

```bash
rg -n '^model:' ~/workspace/queohoh/*/tasks/*/config.yaml
```

- [ ] **Step 2: Apply migrations** (edit files; preserve comments).

- [ ] **Step 3: Remove `goto_command` from `~/workspace/queohoh/config.yaml`.**

- [ ] **Step 4: Commit in the queohoh config repo** (if that tree is its own git):

```bash
cd ~/workspace/queohoh && git add -A && git status
git commit -m "config: multi-provider model backups; drop goto_command/init-tab"
```

If config is not a separate repo, still apply the file edits; note in the queohoh commit message that live config was updated on disk.

- [ ] **Step 5: Verify** after daemon reload: TASKS models re-head under `p` switch; goto no longer types init-tab.

---

### Task 9: Dotfiles — `grok/AGENTS.md` + symlink

**Files:**
- Create: `~/dotfiles/grok/AGENTS.md`
- Symlink: `~/.grok/AGENTS.md` → `~/dotfiles/grok/AGENTS.md`
- Optionally document in `~/dotfiles/docs/installation.md` if Claude symlink is documented there

**Content (minimum):**

```markdown
## Model selection for subagents

When spawning a subagent, set `model:` by how much open-ended judgment remains
— not by task type. A subagent with no `model:` inherits the parent and will
not downgrade on its own, so this choice is load-bearing: pass `model:`
explicitly on every spawn.

- **grok-4.5** — judgment not yet resolved: exploration that feeds a plan/spec,
  unknown-root-cause debugging, code review (bug-finding recall is worth the
  tokens). Default when the answer is still open.
- **composer** — judgment resolved: executing a concrete plan, mechanical edits,
  tests/docs, known-fix bugfixes, narrow "where is X" lookups. Use when the
  task is well-specified AND checkable (tests/build/lint/typecheck) or the
  parent will review. If composer is unavailable, use grok-4.5.

Router: open-ended judgment left? → grok-4.5. No, and something to verify
against → composer. This governs SUBAGENTS only — the parent session model is
set separately (e.g. `/model`).
```

- [ ] **Step 1: Write file + symlink**

```bash
mkdir -p ~/dotfiles/grok
# write AGENTS.md
ln -sfn ~/dotfiles/grok/AGENTS.md ~/.grok/AGENTS.md
ls -la ~/.grok/AGENTS.md
```

- [ ] **Step 2: Commit in dotfiles repo** if applicable:

```bash
cd ~/dotfiles && git add grok/AGENTS.md && git commit -m "feat(grok): AGENTS.md model-selection router (symlink to ~/.grok)"
```

---

### Task 10: Integration pass

**Files:** any stragglers from greps below; docs that still mention `goto_command` / `init-tab` in this repo.

- [ ] **Step 1: Grep cleanup**

```bash
rg -n 'goto_command|gotoCommand|init-tab' packages crates docs --glob '!**/*provider-agnostic*' --glob '!**/*model-catalog*'
rg -n 'def_model_text\(' crates/qoo-tui
```

Every remaining hit must be historical docs or intentional comments.

- [ ] **Step 2: Full gate**

```bash
pnpm --filter @queohoh/core test
pnpm --filter @queohoh/daemon test
cargo test -p qoo-tui
cargo clippy -p qoo-tui -- -D warnings
# or: mise run check
```

All green.

- [ ] **Step 3: Smoke (manual)** after `queohoh reload` / `mise run daemon` + TUI:
  - `p` cycles provider → TASKS Model column re-heads
  - `r` on a def → model dropdown is the chain; default is active provider’s head
  - Session pick shows `claude`/`grok` when known
  - Worktree `g` → provider pick → new tmux tab, shell | agent
  - Queue `g` → resume with correct provider bin
  - `d` → confirm then discover

- [ ] **Step 4: Commit** any remaining integration fixes: `chore: provider-agnostic UX integration pass`

---

## Self-review (plan vs spec)

| Spec section | Task(s) |
|---|---|
| §1 Effective display + config chain | 3 (head); config tab already shows authored Many — verify in Task 3/10 |
| §2 Tiered backups migration | 8 |
| §3 Run picker effective chain | 4 |
| §4 Session provider | 1 + 5 |
| §5 Goto split / kill init-tab / bin | 1 (bin wire) + 2 + 6 |
| §6 Grok AGENTS.md | 9 |
| §7 Discovery confirm | 7 |
| Cron uses active_provider | documented in spec; no code task (already true) |
| Out of scope (Grok session FS, bare tiers, system_prompt) | not planned |

**Placeholder scan:** none intentional.  
**Type consistency:** `SettingsProvider.bin: Option<String>`, `SessionChoice.provider: Option<String>`, `Cmd::Goto { path, cmd }`, `ConfirmAction::DiscoverDef { repo, name }`.
