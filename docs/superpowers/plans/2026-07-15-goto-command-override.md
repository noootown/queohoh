# Workspace-level `goto` command override — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a workspace-level `config.yaml` value (`goto_command`) replace the hardcoded `tmux new-window` that `goto` runs, so both worktree-goto and queue-goto route the operator's own command (e.g. an `init-tab` zsh function) into the new tmux window.

**Architecture:** A new optional `goto_command` string threads config.ts → daemon `StateSnapshot` → TUI `ipc/types.rs` (the exact path `maxConcurrent` already takes). When set, the TUI creates the tmux window and `send-keys` the command into its interactive shell (so shell functions/aliases resolve); a `{cmd}` placeholder becomes `claude --resume <session>` for queue-goto and empty for worktree-goto. When unset, today's behavior is preserved verbatim.

**Tech Stack:** TypeScript (daemon + core, zod schema, vitest), Rust (ratatui TUI, serde, tokio, cargo test).

## Global Constraints

- **Wire-compat is one-directional.** Every new snapshot field must be `Option` in `ipc/types.rs` (the container already has `#[serde(default)]`) and optional in the TS `StateSnapshot` interface, so an old daemon that omits it keeps working. Never remove/rename wire fields.
- **No regression when unset.** Absent `goto_command` → worktree-goto stays `tmux new-window -c <path>`; queue-goto stays `tmux new-window -c <path> 'claude --resume <session>'`, byte-for-byte.
- **No new dependencies** (project convention: side effects hand-rolled, not pulled from crates).
- **TUI is Elm-style:** state changes only in `App::update`; side effects only via `Cmd` variants executed in `event.rs::execute` (fire-and-forget, off the UI thread). The `{cmd}` substitution + tmux orchestration live in `event.rs`; `actions.rs` only copies the raw template onto the `Cmd`.
- **Full gate:** `mise run check` (build, test, typecheck, lint). Per-package: `cargo test -p qoo-tui`, `pnpm -r test`, `pnpm -r typecheck`, `pnpm lint:ci`.
- **Commit convention:** conventional prefixes; do NOT add `Co-Authored-By` trailers. Markdown/docs: one logical line per paragraph/bullet (no hard-wrapping at ~80 cols).

---

## File map

- `packages/core/src/config.ts` — add `goto_command` to schema + `GlobalConfig`; map in `loadGlobalConfig`. (Task 1)
- `packages/core/src/__tests__/config.test.ts` — parse test. (Task 1)
- `packages/daemon/src/api.ts` — add `gotoCommand?` to `StateSnapshot`; populate in `snapshot()`. (Task 2)
- `packages/daemon/src/__tests__/api.test.ts` — snapshot surfaces `gotoCommand`. (Task 2)
- `crates/qoo-tui/src/ipc/types.rs` — add `goto_command: Option<String>` to `StateSnapshot`; two deserialize tests. (Task 3)
- `crates/qoo-tui/src/event.rs` — pure `goto_tmux_plan` + `GotoPlan` enum + unit tests (Task 4); `Cmd` field additions + `run_goto` executor + `execute` arms (Task 5).
- `crates/qoo-tui/src/app/actions.rs` — `goto_worktree`/`goto_queue` copy `snapshot.goto_command` onto the `Cmd`. (Task 5)
- `crates/qoo-tui/src/app/menu_flow_tests.rs`, `crates/qoo-tui/src/app/tests.rs` — update existing `Cmd` match sites; add a wiring test. (Task 5)

---

## Task 1: Config field (`goto_command`) in core config

**Files:**
- Modify: `packages/core/src/config.ts` (schema ~lines 12-37, `GlobalConfig` interface ~39-47, `loadGlobalConfig` return ~68-78)
- Test: `packages/core/src/__tests__/config.test.ts`

**Interfaces:**
- Produces: `GlobalConfig.gotoCommand?: string` — the raw workspace override template (may contain a `{cmd}` placeholder). `undefined` when the key is absent.

- [ ] **Step 1: Write the failing test**

Add to the `describe("loadGlobalConfig", …)` block in `packages/core/src/__tests__/config.test.ts`:

```ts
it("parses goto_command when present and omits it when absent", () => {
	const dir = mkdtempSync(join(tmpdir(), "queohoh-cfg-goto-"));
	const withCmd = join(dir, "with.yaml");
	writeFileSync(
		withCmd,
		["projects: []", 'goto_command: "init-tab {cmd}"'].join("\n"),
	);
	expect(loadGlobalConfig(withCmd).gotoCommand).toBe("init-tab {cmd}");

	const without = join(dir, "without.yaml");
	writeFileSync(without, "projects: []\n");
	expect(loadGlobalConfig(without).gotoCommand).toBeUndefined();
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm --filter @queohoh/core test -- config.test`
Expected: FAIL — `gotoCommand` is `undefined` on the with-cmd case (property does not exist yet / not mapped).

- [ ] **Step 3: Add the schema key, interface field, and mapping**

In `packages/core/src/config.ts`, inside `GlobalConfigSchema`'s `z.object({ … })` (alongside `vars`/`models`), add:

```ts
		// A line of shell typed into the tmux window that `goto` opens (worktree-
		// goto and queue-goto). The `{cmd}` placeholder is substituted downstream:
		// the `claude --resume <session>` command for queue-goto, empty for
		// worktree-goto. Absent → the TUI keeps its built-in `tmux new-window`
		// behavior. NOTE: a template without `{cmd}` means queue-goto will not
		// resume Claude (nothing to substitute the resume command into).
		goto_command: z.string().optional(),
```

Add to the `GlobalConfig` interface:

```ts
	/** Workspace-level override for the command `goto` runs — see the schema. */
	gotoCommand?: string;
```

Add to the object returned by `loadGlobalConfig` (alongside `vars`/`models`):

```ts
		gotoCommand: config.goto_command,
```

- [ ] **Step 4: Run test to verify it passes**

Run: `pnpm --filter @queohoh/core test -- config.test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add packages/core/src/config.ts packages/core/src/__tests__/config.test.ts
git commit -m "feat(core): workspace goto_command config field"
```

---

## Task 2: Surface `gotoCommand` in the daemon snapshot

**Files:**
- Modify: `packages/daemon/src/api.ts` (`StateSnapshot` interface ~39-61; `snapshot()` return ~110-126)
- Test: `packages/daemon/src/__tests__/api.test.ts` (`setup` opts ~38-43 + config literal ~58-64; new test near the "state snapshot exposes projects" test ~147)

**Interfaces:**
- Consumes: `GlobalConfig.gotoCommand` (Task 1).
- Produces: `StateSnapshot.gotoCommand?: string` on the wire (JSON key `gotoCommand`; dropped from JSON when `undefined`).

- [ ] **Step 1: Write the failing test**

In `packages/daemon/src/__tests__/api.test.ts`, extend the `setup` opts type (the object at ~lines 38-43) with:

```ts
	gotoCommand?: string;
```

and add `gotoCommand: opts?.gotoCommand,` to the `config: GlobalConfig = { … }` literal (~lines 58-64), alongside `vars`/`models`.

Then add a test inside `describe("ApiServer", …)`:

```ts
	it("surfaces gotoCommand from config in the state snapshot", async () => {
		const { client } = await setup({ gotoCommand: "init-tab {cmd}" });
		const state = (await client.call("state")) as { gotoCommand?: string };
		expect(state.gotoCommand).toBe("init-tab {cmd}");
	});

	it("omits gotoCommand when config has none", async () => {
		const { client } = await setup();
		const state = (await client.call("state")) as { gotoCommand?: string };
		expect(state.gotoCommand).toBeUndefined();
	});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm --filter @queohoh/daemon test -- api.test`
Expected: FAIL — `state.gotoCommand` is `undefined` on the with-cmd case (snapshot does not carry it yet). The opts/config edits are needed to compile the test.

- [ ] **Step 3: Add the field to the interface and populate it**

In `packages/daemon/src/api.ts`, add to the `StateSnapshot` interface (after `buildId?`):

```ts
	/**
	 * Workspace-level override for the command `goto` opens in the new tmux
	 * window (see GlobalConfig.gotoCommand). Optional/additive — absent config
	 * omits the field, old TUIs ignore it, and the TUI falls back to its
	 * built-in `tmux new-window` behavior.
	 */
	gotoCommand?: string;
```

In `snapshot()`, add before the closing `}` of the returned object (after `buildId: this.buildId,`):

```ts
			gotoCommand: this.deps.config.gotoCommand,
```

- [ ] **Step 4: Run test to verify it passes**

Run: `pnpm --filter @queohoh/daemon test -- api.test`
Expected: PASS (both new tests green).

- [ ] **Step 5: Commit**

```bash
git add packages/daemon/src/api.ts packages/daemon/src/__tests__/api.test.ts
git commit -m "feat(daemon): carry gotoCommand in state snapshot"
```

---

## Task 3: Mirror `goto_command` on the TUI wire type

**Files:**
- Modify: `crates/qoo-tui/src/ipc/types.rs` (`StateSnapshot` struct ~18-35; test module `mod tests` starts ~341)

**Interfaces:**
- Consumes: the `gotoCommand` JSON key from Task 2.
- Produces: `StateSnapshot.goto_command: Option<String>` — read by `actions.rs` in Task 5.

- [ ] **Step 1: Write the failing test**

Add inside the `#[cfg(test)] mod tests { … }` block in `crates/qoo-tui/src/ipc/types.rs`:

```rust
    #[test]
    fn goto_command_present_deserializes_to_some() {
        let s: StateSnapshot =
            serde_json::from_str(r#"{"gotoCommand":"init-tab {cmd}"}"#).unwrap();
        assert_eq!(s.goto_command.as_deref(), Some("init-tab {cmd}"));
    }

    #[test]
    fn goto_command_absent_deserializes_to_none() {
        let s: StateSnapshot = serde_json::from_str("{}").unwrap();
        assert_eq!(s.goto_command, None);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p qoo-tui goto_command_ -- --nocapture`
Expected: FAIL to compile — no field `goto_command` on `StateSnapshot`.

- [ ] **Step 3: Add the field**

In `crates/qoo-tui/src/ipc/types.rs`, add to the `StateSnapshot` struct after `pub build_id: Option<String>,`:

```rust
    /// Workspace-level override for the command `goto` runs (its `gotoCommand`
    /// on the wire — see api.ts). `None` on an old daemon that omits it (via the
    /// container `default`) or when no override is configured; then the TUI
    /// keeps its built-in `tmux new-window` behavior. `{cmd}` inside is
    /// substituted to `claude --resume <session>` (queue-goto) or empty
    /// (worktree-goto) when the window is driven in `event.rs`.
    pub goto_command: Option<String>,
```

The container `#[serde(rename_all = "camelCase", default)]` maps `gotoCommand` → `goto_command` and fills `None` when absent — no `deserialize_with` needed.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p qoo-tui goto_command_`
Expected: PASS (both tests).

- [ ] **Step 5: Commit**

```bash
git add crates/qoo-tui/src/ipc/types.rs
git commit -m "feat(tui): mirror goto_command on StateSnapshot wire type"
```

---

## Task 4: Pure `goto_tmux_plan` planner

**Files:**
- Modify: `crates/qoo-tui/src/event.rs` (add the enum, function, and a `#[cfg(test)] mod goto_plan_tests`)

**Interfaces:**
- Produces:
  - `enum GotoPlan { Simple { args: Vec<String> }, CreateAndSend { new_window_args: Vec<String>, send_line: String } }`
  - `fn goto_tmux_plan(path: &str, session_id: Option<&str>, goto_command: Option<&str>) -> GotoPlan` — `session_id` = `Some` for queue-goto (resume), `None` for worktree-goto; `goto_command` = the raw workspace template. Used by `run_goto`/`execute` in Task 5.

- [ ] **Step 1: Write the failing tests**

Add near the bottom of `crates/qoo-tui/src/event.rs`:

```rust
#[cfg(test)]
mod goto_plan_tests {
    use super::{goto_tmux_plan, GotoPlan};

    fn v(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn worktree_no_override_is_plain_new_window() {
        assert_eq!(
            goto_tmux_plan("/wt/a", None, None),
            GotoPlan::Simple { args: v(&["new-window", "-c", "/wt/a"]) }
        );
    }

    #[test]
    fn queue_no_override_appends_resume_command() {
        assert_eq!(
            goto_tmux_plan("/wt/a", Some("sess1"), None),
            GotoPlan::Simple {
                args: v(&["new-window", "-c", "/wt/a", "claude --resume sess1"])
            }
        );
    }

    #[test]
    fn worktree_override_substitutes_empty_cmd() {
        assert_eq!(
            goto_tmux_plan("/wt/a", None, Some("init-tab {cmd}")),
            GotoPlan::CreateAndSend {
                new_window_args: v(&[
                    "new-window", "-P", "-F", "#{window_id}", "-c", "/wt/a"
                ]),
                send_line: "init-tab ".to_string(),
            }
        );
    }

    #[test]
    fn queue_override_substitutes_resume_command() {
        assert_eq!(
            goto_tmux_plan("/wt/a", Some("sess1"), Some("init-tab {cmd}")),
            GotoPlan::CreateAndSend {
                new_window_args: v(&[
                    "new-window", "-P", "-F", "#{window_id}", "-c", "/wt/a"
                ]),
                send_line: "init-tab claude --resume sess1".to_string(),
            }
        );
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p qoo-tui goto_plan_tests`
Expected: FAIL to compile — `goto_tmux_plan`/`GotoPlan` undefined.

- [ ] **Step 3: Add the enum and function**

Add to `crates/qoo-tui/src/event.rs` (module scope, near `open_tmux_window`):

```rust
/// The tmux invocation shape for a `goto`, derived purely from the target and
/// the optional workspace `goto_command` override. `event.rs` executes it; the
/// split keeps the (untested) tmux side effects thin and this derivation unit-
/// tested.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum GotoPlan {
    /// No override: a single, fully-formed `tmux new-window …` invocation —
    /// byte-for-byte today's behavior.
    Simple { args: Vec<String> },
    /// Override present: create a window capturing `#{window_id}`, then type
    /// `send_line` into it so the operator's interactive-shell functions/aliases
    /// resolve. `event.rs` substitutes the real window id (from the first
    /// invocation's stdout) into the follow-up `send-keys -t <id>` calls.
    CreateAndSend { new_window_args: Vec<String>, send_line: String },
}

/// Build the goto plan. `session_id` = `Some` for a queue goto (resume Claude),
/// `None` for a worktree goto. The template's `{cmd}` placeholder becomes the
/// resume command (queue) or the empty string (worktree).
pub(crate) fn goto_tmux_plan(
    path: &str,
    session_id: Option<&str>,
    goto_command: Option<&str>,
) -> GotoPlan {
    let cmd = match session_id {
        Some(id) => format!("claude --resume {id}"),
        None => String::new(),
    };
    match goto_command {
        Some(template) => GotoPlan::CreateAndSend {
            new_window_args: vec![
                "new-window".into(),
                "-P".into(),
                "-F".into(),
                "#{window_id}".into(),
                "-c".into(),
                path.into(),
            ],
            send_line: template.replace("{cmd}", &cmd),
        },
        None => {
            let mut args = vec!["new-window".into(), "-c".into(), path.into()];
            if !cmd.is_empty() {
                args.push(cmd);
            }
            GotoPlan::Simple { args }
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p qoo-tui goto_plan_tests`
Expected: PASS (4 tests). A `dead_code` warning on the unused items is acceptable here — Task 5 wires them; if the crate denies warnings and the build fails, proceed straight into Task 5 in the same session (the two land together).

- [ ] **Step 5: Commit**

```bash
git add crates/qoo-tui/src/event.rs
git commit -m "feat(tui): pure goto_tmux_plan planner + tests"
```

---

## Task 5: Wire the override through `Cmd`, `event.rs`, and `actions.rs`

This is the atomic compile unit: adding a field to `Cmd::OpenTmux`/`Cmd::TmuxResume` ripples to their one construction site each (`actions.rs`) and their match sites (tests). All change together.

**Files:**
- Modify: `crates/qoo-tui/src/event.rs` (`Cmd` enum ~115-119; `execute` arms ~470-502; add `run_goto`)
- Modify: `crates/qoo-tui/src/app/actions.rs` (`goto_worktree` ~892; `goto_queue` ~924-926)
- Modify: `crates/qoo-tui/src/app/menu_flow_tests.rs` (lines 419, 627, 684) and `crates/qoo-tui/src/app/tests.rs` (lines 481, 736)

**Interfaces:**
- Consumes: `goto_tmux_plan`/`GotoPlan` (Task 4), `StateSnapshot.goto_command` (Task 3).
- Produces: `Cmd::OpenTmux { path: String, goto_command: Option<String> }` and `Cmd::TmuxResume { path: String, session_id: String, goto_command: Option<String> }`.

- [ ] **Step 1: Write the failing wiring test**

Add to `crates/qoo-tui/src/app/menu_flow_tests.rs` (near `g_on_worktree_row_opens_tmux_when_inside_tmux`):

```rust
#[test]
fn g_on_worktree_threads_goto_command_into_open_tmux() {
    let snap = StateSnapshot {
        goto_command: Some("init-tab {cmd}".into()),
        ..worktree_snapshot()
    };
    let mut a = app_with(snap);
    a.inside_tmux = true;
    focus_worktrees(&mut a);
    let up = a.update(key('g'));
    assert!(matches!(&up.cmds[..],
        [Cmd::OpenTmux { path, goto_command }]
        if path == "/wt/wt-a" && goto_command.as_deref() == Some("init-tab {cmd}")));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p qoo-tui g_on_worktree_threads_goto_command`
Expected: FAIL to compile — `Cmd::OpenTmux` has no `goto_command` field.

- [ ] **Step 3: Add the fields, executor, and construction**

**3a — `crates/qoo-tui/src/event.rs`, `Cmd` enum** (~lines 115-119), replace the two variants:

```rust
    OpenTmux { path: String, goto_command: Option<String> },
    /// Resume a task's Claude session in a NEW tmux tab (window) rooted at its
    /// worktree. With no `goto_command`, runs `tmux new-window -c <path>
    /// 'claude --resume <session_id>'`; with one, opens a plain window and types
    /// the (substituted) command into it. Fired by the queue "Resume"/goto
    /// action; gated on being inside tmux + a known session/path.
    TmuxResume { path: String, session_id: String, goto_command: Option<String> },
```

**3b — `crates/qoo-tui/src/event.rs`, `run_goto` executor.** Add near `open_tmux_window`:

```rust
/// Execute a [`GotoPlan`] off the UI thread. On any tmux failure, reports the
/// stderr as a status line (mirrors `open_tmux_window`); success is silent.
async fn run_goto(plan: GotoPlan, tx: UnboundedSender<Event>) {
    async fn tmux(args: &[&str]) -> Result<std::process::Output, std::io::Error> {
        tokio::process::Command::new("tmux").args(args).output().await
    }
    let status = match plan {
        GotoPlan::Simple { args } => {
            let argv: Vec<&str> = args.iter().map(String::as_str).collect();
            match tmux(&argv).await {
                Ok(out) if out.status.success() => None,
                Ok(out) => Some(format!("tmux: {}", String::from_utf8_lossy(&out.stderr).trim())),
                Err(e) => Some(format!("tmux: {e}")),
            }
        }
        GotoPlan::CreateAndSend { new_window_args, send_line } => {
            let argv: Vec<&str> = new_window_args.iter().map(String::as_str).collect();
            match tmux(&argv).await {
                Ok(out) if out.status.success() => {
                    let win = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    // `-l` = literal keys (no key-name lookup on the text); `--`
                    // guards a leading '-'. Enter is a separate key-name call.
                    let _ = tmux(&["send-keys", "-t", &win, "-l", "--", &send_line]).await;
                    let _ = tmux(&["send-keys", "-t", &win, "Enter"]).await;
                    None
                }
                Ok(out) => Some(format!("tmux: {}", String::from_utf8_lossy(&out.stderr).trim())),
                Err(e) => Some(format!("tmux: {e}")),
            }
        }
    };
    if let Some(status) = status {
        let _ = tx.send(Event::ActionResult { status: Some(status), invalidate_defs_for: None });
    }
}
```

**3c — `crates/qoo-tui/src/event.rs`, `execute` arms.** Replace the whole `Cmd::OpenTmux { … } => { … }` and `Cmd::TmuxResume { … } => { … }` arms (~470-502) with:

```rust
        Cmd::OpenTmux { path, goto_command } => {
            tokio::spawn(run_goto(
                goto_tmux_plan(&path, None, goto_command.as_deref()),
                tx,
            ));
        }
        Cmd::TmuxResume { path, session_id, goto_command } => {
            tokio::spawn(run_goto(
                goto_tmux_plan(&path, Some(&session_id), goto_command.as_deref()),
                tx,
            ));
        }
```

(This replaces the old inline `tmux new-window` bodies; the no-override `GotoPlan::Simple` reproduces them exactly.)

**3d — `crates/qoo-tui/src/app/actions.rs`, `goto_worktree`** (~line 888-892). Replace the `let Some(row) = …` tail through the `Update { … }` with:

```rust
        let Some(row) = self.selected_worktree_row_filtered() else {
            self.status_line = Some("no worktree selected".into());
            return Update { dirty: true, cmds: vec![] };
        };
        let path = row.path.clone();
        let goto_command = self.snapshot.as_ref().and_then(|s| s.goto_command.clone());
        Update { dirty: true, cmds: vec![Cmd::OpenTmux { path, goto_command }] }
```

**3e — `crates/qoo-tui/src/app/actions.rs`, `goto_queue`** (~line 924). Replace the `QueueGotoTarget::Ready` arm body:

```rust
            QueueGotoTarget::Ready(session_id, path) => {
                let goto_command = self.snapshot.as_ref().and_then(|s| s.goto_command.clone());
                Update { dirty: true, cmds: vec![Cmd::TmuxResume { path, session_id, goto_command }] }
            }
```

- [ ] **Step 4: Update the existing `Cmd` match sites**

These `matches!` patterns must accept the new field. Change each:

- `crates/qoo-tui/src/app/menu_flow_tests.rs:419` — `[Cmd::TmuxResume { path, session_id }]` → `[Cmd::TmuxResume { path, session_id, .. }]`
- `crates/qoo-tui/src/app/menu_flow_tests.rs:627` — `[Cmd::OpenTmux { path }]` → `[Cmd::OpenTmux { path, .. }]`
- `crates/qoo-tui/src/app/menu_flow_tests.rs:684` — `[Cmd::OpenTmux { path }]` → `[Cmd::OpenTmux { path, .. }]`
- `crates/qoo-tui/src/app/tests.rs:481` — `[Cmd::TmuxResume { path, session_id }]` → `[Cmd::TmuxResume { path, session_id, .. }]`
- `crates/qoo-tui/src/app/tests.rs:736` — `[Cmd::TmuxResume { path, session_id }]` → `[Cmd::TmuxResume { path, session_id, .. }]`

(`bulk_flow_tests.rs:202` already uses `Cmd::OpenTmux { .. }` — no change.)

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p qoo-tui`
Expected: PASS — the new wiring test, the four planner tests, the two deserialize tests, and all previously-green tests (with updated match sites).

- [ ] **Step 6: Full gate + commit**

Run: `mise run check`
Expected: build + all tests + typecheck + lint green.

```bash
git add crates/qoo-tui/src/event.rs crates/qoo-tui/src/app/actions.rs \
        crates/qoo-tui/src/app/menu_flow_tests.rs crates/qoo-tui/src/app/tests.rs
git commit -m "feat(tui): route goto through workspace goto_command override"
```

---

## Manual verification (after Task 5)

1. In the workspace `config.yaml`, set `goto_command: "init-tab {cmd}"` and ensure `init-tab` handles an optional trailing command (e.g. `init-tab () { [ -n "$1" ] && tmux send-keys "$*" Enter; tmux split-window -h; … }`).
2. Rebuild + restart the daemon (`mise run daemon`), open the TUI inside tmux.
3. WORKTREES → `g` on a worktree → a new window opens rooted at the worktree and your `init-tab` setup runs (nvim/split/etc.), no `claude` resume.
4. QUEUE → `g` on a task with a recorded session → a new window opens and `init-tab claude --resume <id>` runs (Claude resumes inside your layout).
5. Remove `goto_command` from `config.yaml`, restart the daemon → both gotos behave exactly as before (plain shell / bare `claude --resume`).

---

## Self-review notes

- **Spec coverage:** config surface → Task 1; daemon snapshot → Task 2; wire type → Task 3; mechanism (`{cmd}` substitution, `new-window -P -F` + `send-keys -l`) → Tasks 4-5; no-regression default → `GotoPlan::Simple` (Tasks 4-5) + manual step 5; pure planner + four test cases → Task 4; sharp edges documented → Task 1 schema comment.
- **Type consistency:** `GotoPlan` / `goto_tmux_plan` signatures identical across Tasks 4-5; `Cmd::OpenTmux { path, goto_command }` and `Cmd::TmuxResume { path, session_id, goto_command }` consistent between the enum def (3a), constructions (3d/3e), executor (3c), and match sites (Step 4); `gotoCommand` (TS) ↔ `goto_command` (Rust) mapping via container `camelCase`.
