# Explicit Discover Verb Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `r` on the TASKS pane always plain-runs a definition (fixing the `has no discovery` error on zero-arg cron defs); a new `d` verb explicitly runs discovery fan-out; discovery-backed defs get a `⌕` row marker and a `discovery` detail sub-tab.

**Architecture:** The daemon's `runDefinition` RPC drops its "no args → discover" inference and always uses args mode; a new `discoverDefinition` RPC carries the explicit discover trigger. The TUI adds a `Discover` pane chip + `d` key on TASKS (gated by the existing chip/keymap single-source-of-truth), a schedule-column marker, and a third definition detail sub-tab. The cron engine is untouched.

**Tech Stack:** TypeScript (pnpm workspace: `packages/daemon`, vitest), Rust (ratatui TUI: `crates/qoo-tui`, cargo + insta snapshots).

**Spec:** `docs/superpowers/specs/2026-07-14-explicit-discover-verb-design.md`

## Global Constraints

- No dedup behavior changes anywhere (`instantiate.ts:91-92` cron bypass stays as-is).
- Cron engine trigger selection (`packages/daemon/src/engine.ts:396`) stays as-is.
- No MCP discover tool; `run_task_definition` keeps its schema, only its description text changes.
- Rust: `selectors.rs` holds pure unit-testable derivations; view files stay free of business logic (repo AGENTS.md rule).
- Commit messages: conventional prefix, no Co-Authored-By trailers.
- Verification: `pnpm -C packages/daemon test` (vitest), `cargo test -p qoo-tui`. Insta snapshot diffs must be reviewed, not blindly accepted.

---

### Task 1: Daemon — `runDefinition` always args mode + new `discoverDefinition` RPC

**Files:**
- Modify: `packages/daemon/src/api.ts` (the `runDefinition` case, ~line 524-528; new `discoverDefinition` case after `definition`, ~line 567)
- Modify: `packages/daemon/src/mcp.ts` (the `run_task_definition` tool description, ~line 180)
- Test: `packages/daemon/src/__tests__/api.test.ts`

**Interfaces:**
- Consumes: `instantiateDefinition(def, trigger, deps)` from `@core/instantiate` (already imported in api.ts); `resolveDefinition`, `projectWorkspaceDir`, `loadProjectVars`, `defaultExec` (already imported).
- Produces: RPC method `discoverDefinition` with params `{repo: string, name: string, source?: "mcp"|"tui"}` returning `TaskInstance[]` — Task 2's TUI command calls this exact method name. `runDefinition` semantics: trigger is always `{mode: "args", values: args.map(String)}`.

- [ ] **Step 1: Write the failing tests**

Add to `packages/daemon/src/__tests__/api.test.ts`, inside the same `describe` that holds the existing `runDefinition` tests (near line 577). First a fixture helper next to `writeAutoDef` (~line 282):

```ts
// A zero-arg def with a discovery block. Plain run (r) must create exactly ONE
// task from the static prompt; discover (d) must fan out one task per item the
// discovery command prints.
function writeDiscoveryDef(workspace: string): void {
	const dir = join(workspace, "platform", "tasks", "sweep");
	mkdirSync(dir, { recursive: true });
	writeFileSync(
		join(dir, "config.yaml"),
		'discovery:\n  command: echo \'[{"n":"1"},{"n":"2"}]\'\n  item_key: "{{n}}"\ndedup: none\n',
	);
	writeFileSync(join(dir, "prompt.md"), "Static run.\n");
}

// The bug's regression fixture: zero args, no discovery — the shape of a plain
// cron def (slack-react-release-notes / workspace-sanitize).
function writePlainZeroArgDef(workspace: string): void {
	const dir = join(workspace, "platform", "tasks", "daily");
	mkdirSync(dir, { recursive: true });
	writeFileSync(join(dir, "config.yaml"), "dedup: none\n");
	writeFileSync(join(dir, "prompt.md"), "Do the daily thing.\n");
}
```

Then the four tests:

```ts
it("runDefinition with zero args on a no-discovery def plain-runs (regression: 'has no discovery')", async () => {
	const { client, workspace } = await setup();
	writePlainZeroArgDef(workspace);
	const created = (await client.call("runDefinition", {
		repo: "platform",
		name: "daily",
		args: [],
	})) as { prompt: string }[];
	expect(created).toHaveLength(1);
	expect(created[0]?.prompt).toBe("Do the daily thing.\n");
});

it("runDefinition with zero args on a DISCOVERY def plain-runs (never discovers implicitly)", async () => {
	const { client, workspace } = await setup();
	writeDiscoveryDef(workspace);
	const created = (await client.call("runDefinition", {
		repo: "platform",
		name: "sweep",
		args: [],
	})) as { prompt: string }[];
	// Discovery would fan out 2 tasks; a plain run creates exactly 1.
	expect(created).toHaveLength(1);
	expect(created[0]?.prompt).toBe("Static run.\n");
});

it("discoverDefinition runs discovery and fans out one task per item", async () => {
	const { client, workspace } = await setup();
	writeDiscoveryDef(workspace);
	const created = (await client.call("discoverDefinition", {
		repo: "platform",
		name: "sweep",
	})) as { prompt: string; source: string; itemKey: string }[];
	expect(created).toHaveLength(2);
	expect(created.map((t) => t.itemKey).sort()).toEqual(["1", "2"]);
	expect(created[0]?.source).toBe("tui");
});

it("discoverDefinition on a def without discovery rejects", async () => {
	const { client, workspace } = await setup();
	writePlainZeroArgDef(workspace);
	await expect(
		client.call("discoverDefinition", { repo: "platform", name: "daily" }),
	).rejects.toThrow(/has no discovery/);
});
```

Note: if `TaskInstance`'s item-key field is named differently on the wire (check an existing test or `packages/core/src/task.ts` — it is `itemKey` in `store.create`), adjust the assertion to the real field name rather than weakening it.

- [ ] **Step 2: Run tests to verify they fail**

Run: `pnpm -C packages/daemon test`
Expected: the two `discoverDefinition` tests FAIL with an unknown-method error, and the two `runDefinition` tests FAIL — zero-args currently routes to discover mode (`has no discovery` for `daily`; 2 tasks instead of 1 for `sweep`).

- [ ] **Step 3: Implement**

In `packages/daemon/src/api.ts`, replace the trigger ternary in the `runDefinition` case (lines 524-528):

```ts
				const created = await instantiateDefinition(
					def,
					// Always args mode: zero args fill from declared defaults, a
					// required arg without a default errors with `missing required
					// arg`. Discovery is an explicit verb — `discoverDefinition`.
					{ mode: "args", values: args.map(String) },
					{
```

(The rest of the deps object is unchanged.)

Add a new case directly after `case "definition"` (after ~line 567):

```ts
				case "discoverDefinition": {
					const repo = String(params.repo ?? "");
					const name = String(params.name ?? "");
					const project = deps.config.projects.find((p) => p.name === repo);
					if (!project) throw new Error(`unknown repo: ${repo}`);
					const projectDir = projectWorkspaceDir(deps.config, repo);
					const def = resolveDefinition(deps.config, repo, name);
					const source = params.source === "mcp" ? "mcp" : "tui";
					// The explicit discover verb: run the def's discovery command and
					// fan out one task per fresh item. No worktree/ref/cwd overrides —
					// each item resolves its own ref via the def's `worktree:` setting.
					// `instantiateDefinition` rejects a def without discovery.
					const created = await instantiateDefinition(
						def,
						{ mode: "discover" },
						{
							store: deps.store,
							exec: defaultExec,
							cwd: projectDir,
							source,
							globalVars: {
								project: repo,
								repo_path: project.path,
								...deps.config.vars,
							},
							repoVars: loadProjectVars(projectDir),
						},
					);
					deps.onMutation();
					return created;
				}
```

In `packages/daemon/src/mcp.ts`, update the `run_task_definition` description (line 180) — remove the "Without args, runs the definition's discovery command…" sentence:

```ts
			"Trigger a task definition as a plain run. Args fill the definition's declared args positionally; trailing args fall back to their declared defaults, and a required arg without a default is an error. Target precedence is cwd > worktree > ref > the definition's own worktree: setting; pass ref to pin the target (e.g. ref 'temp') and override a 'worktree: auto' definition that would otherwise target a PR/ticket URL found in the args. Returns created tasks as JSON.",
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `pnpm -C packages/daemon test`
Expected: all 4 new tests PASS. Also confirm no pre-existing test regressed — in particular any test that relied on no-args `runDefinition` discovering (search the file for `runDefinition` calls without `args`; per exploration there are none, all pass args).

- [ ] **Step 5: Typecheck + lint**

Run: `pnpm -r typecheck && pnpm lint:ci`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add packages/daemon/src/api.ts packages/daemon/src/mcp.ts packages/daemon/src/__tests__/api.test.ts
git commit -m "feat(daemon): runDefinition always args mode; explicit discoverDefinition RPC"
```

---

### Task 2: TUI — `d` discover verb (chip, key, action, mouse)

**Files:**
- Modify: `crates/qoo-tui/src/view/theme.rs` (add `BTN_LABEL_DISCOVER` next to `BTN_LABEL_RUN`, ~line 89)
- Modify: `crates/qoo-tui/src/hit.rs` (`PaneButton` enum ~line 21; `pane_buttons` ~line 38)
- Modify: `crates/qoo-tui/src/keymap.rs` (`AppAction` enum ~line 51; key match ~line 132)
- Modify: `crates/qoo-tui/src/view/panes.rs` (`button_chip` match ~line 99)
- Modify: `crates/qoo-tui/src/app/actions.rs` (dispatch arm ~line 221; new methods near `run_selected_task_def` ~line 590 and `run_definition_cmd` ~line 628)
- Modify: `crates/qoo-tui/src/app/mouse.rs` (chip-click match ~line 458)
- Test: `crates/qoo-tui/src/keymap.rs` (tests module), `crates/qoo-tui/src/app/menu_flow_tests.rs`

**Interfaces:**
- Consumes: RPC method `discoverDefinition` `{repo, name, source: "tui"}` from Task 1; `DefinitionSummary.has_discovery: bool` (exists, `ipc/types.rs:207`); existing helpers `filter_rows`, `is_bulk_selection`, `Cmd::Rpc`, `RpcCall`.
- Produces: `AppAction::DiscoverSelectedDef`, `PaneButton::Discover`, `App::discover_selected_def()`, `App::discover_definition_cmd(repo, name) -> Cmd` — Task 3/4 do not depend on these; nothing else consumes them.

- [ ] **Step 1: Write the failing tests**

In `crates/qoo-tui/src/keymap.rs` tests module (next to `r_runs_def_on_tasks_requeues_on_queue_new_task_on_worktrees`, ~line 391):

```rust
#[test]
fn d_discovers_on_tasks_only() {
    assert_eq!(
        list_mode_action(&k(KeyCode::Char('d')), PaneId::Tasks),
        AppAction::DiscoverSelectedDef
    );
    // No Discover chip on QUEUE / WORKTREES → the gate leaves `d` inert there.
    assert_eq!(list_mode_action(&k(KeyCode::Char('d')), PaneId::Queue), AppAction::None);
    assert_eq!(list_mode_action(&k(KeyCode::Char('d')), PaneId::Worktrees), AppAction::None);
}
```

In `crates/qoo-tui/src/app/menu_flow_tests.rs` (next to `tasks_pane_run_zero_arg_def_dispatches_and_closes`, ~line 643 — reuse its snapshot/fixture pattern):

```rust
#[test]
fn tasks_pane_d_dispatches_discover_for_a_discovery_def() {
    let snap = StateSnapshot {
        projects: vec![Project { name: "platform".into(), github_id: None }],
        ..Default::default()
    };
    let mut a = app_with(snap);
    a.defs_by_project.insert("platform".into(), vec![{
        let mut d = crate::ipc::types::DefinitionSummary::default();
        d.repo = "platform".into();
        d.name = "pr-review".into();
        d.has_discovery = true;
        d
    }]);
    focus_tasks(&mut a);
    let u = a.update(key('d'));
    assert!(matches!(a.mode, Mode::List));
    assert!(
        u.cmds.iter().any(|c| matches!(c, Cmd::Rpc { call, invalidate_defs_for, .. }
            if call.method == "discoverDefinition"
                && call.params["name"] == "pr-review"
                && call.params["source"] == "tui"
                && invalidate_defs_for.as_deref() == Some("platform"))),
        "expected a discoverDefinition dispatch, got {:?}",
        u.cmds,
    );
}

#[test]
fn tasks_pane_d_on_a_no_discovery_def_sets_status_line_no_rpc() {
    let snap = StateSnapshot {
        projects: vec![Project { name: "platform".into(), github_id: None }],
        ..Default::default()
    };
    let mut a = app_with(snap);
    a.defs_by_project.insert("platform".into(), vec![{
        let mut d = crate::ipc::types::DefinitionSummary::default();
        d.repo = "platform".into();
        d.name = "lint".into();
        // has_discovery: false (default)
        d
    }]);
    focus_tasks(&mut a);
    let u = a.update(key('d'));
    assert!(u.cmds.is_empty(), "no RPC for a def without discovery");
    assert_eq!(a.status_line.as_deref(), Some("lint has no discovery"));
}
```

If `DefinitionSummary` does not implement `Default`, build it field-by-field exactly as `tasks_pane_run_zero_arg_def_dispatches_and_closes` does (copy that test's construction).

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p qoo-tui d_discovers_on_tasks_only`
Expected: COMPILE ERROR (`DiscoverSelectedDef` not defined) — a compile failure is the failing state here.

- [ ] **Step 3: Implement**

`crates/qoo-tui/src/view/theme.rs` (next to `BTN_LABEL_RUN`, line 89):

```rust
pub const BTN_LABEL_DISCOVER: &str = "discover";
```

`crates/qoo-tui/src/hit.rs` — add the variant to `PaneButton` (enum at ~line 21, after `Run` or alphabetically consistent with the file):

```rust
    Discover,
```

and give TASKS the chip (`pane_buttons`, ~line 42):

```rust
        PaneId::Tasks => &[Run, Discover, Collapse],
```

`bulk_allowed` is untouched — `Discover` is single-row only, so a bulk selection dims the chip and refuses via the existing machinery. Update the chip-set comment in `keymap.rs` (~line 127) from `TASKS {r,z}` to `TASKS {r,d,z}`.

`crates/qoo-tui/src/keymap.rs` — `AppAction` variant (next to `RunSelectedDef`, ~line 51):

```rust
    /// Run the TASKS pane's highlighted definition's DISCOVERY (`d`, and the
    /// tasks `[d]iscover` chip): fan out one task per discovered item. Defs
    /// without a discovery block refuse with a status line. Routes to
    /// `App::discover_selected_def`.
    DiscoverSelectedDef,
```

Key arm (after the `r` match, ~line 136):

```rust
        // `d` is a TASKS-only chip: run the highlighted def's discovery fan-out.
        KeyCode::Char('d') => gated(PaneButton::Discover, AppAction::DiscoverSelectedDef),
```

`crates/qoo-tui/src/view/panes.rs` — `button_chip` match (~line 102, import `BTN_LABEL_DISCOVER` in the theme import list at line 21):

```rust
        PaneButton::Discover => ('d', BTN_LABEL_DISCOVER),
```

`crates/qoo-tui/src/app/actions.rs` — dispatch arm (next to `A::RunSelectedDef`, ~line 221):

```rust
            A::DiscoverSelectedDef => {
                // `d` is a TASKS chip (keymap-gated there): explicit discovery
                // fan-out for the highlighted def. Single-row only.
                let u = self.discover_selected_def();
                cmds.extend(u.cmds);
                u.dirty
            }
```

New methods (next to `run_selected_task_def` / `run_definition_cmd`):

```rust
    /// `d` on TASKS (and the `[d]iscover` chip): run the highlighted def's
    /// discovery command daemon-side and fan out one task per item. Mirrors
    /// [`Self::run_selected_task_def`]'s selection resolution; a def without a
    /// discovery block refuses with a status line (no RPC), and a bulk range
    /// refuses like every non-bulk verb.
    pub(super) fn discover_selected_def(&mut self) -> Update {
        let ui = self.active_ui();
        let sel = ui.selections[ListPane::Tasks.idx()];
        let marks = &ui.marks[ListPane::Tasks.idx()];
        if crate::view::is_bulk_selection(&sel, marks) {
            self.status_line = Some(BULK_NOT_APPLICABLE.into());
            return Update { dirty: true, cmds: vec![] };
        }
        let Some(repo) = self.active_repo() else {
            return Update { dirty: false, cmds: vec![] };
        };
        let ui = self.active_ui();
        let defs = self.defs_by_project.get(&repo).cloned().unwrap_or_default();
        let vis = crate::selectors::filter_rows(&defs, &ui.search[ListPane::Tasks.idx()], |d| d.name.clone());
        let cursor = ui.selections[ListPane::Tasks.idx()].cursor.min(vis.len().saturating_sub(1));
        let Some(def) = vis.get(cursor).and_then(|&i| defs.get(i)).cloned() else {
            self.status_line = Some("nothing selected".into());
            return Update { dirty: true, cmds: vec![] };
        };
        if !def.has_discovery {
            self.status_line = Some(format!("{} has no discovery", def.name));
            return Update { dirty: true, cmds: vec![] };
        }
        Update { dirty: true, cmds: vec![Self::discover_definition_cmd(&def.repo, &def.name)] }
    }

    /// Build the fire-and-forget `discoverDefinition` command. Same client
    /// contract as [`Self::run_definition_cmd`]: timeout is treated as success
    /// (discovery can outlive it; the push subscription re-syncs) and a
    /// successful call invalidates the repo's def summaries.
    pub(super) fn discover_definition_cmd(repo: &str, name: &str) -> Cmd {
        Cmd::Rpc {
            label: "discover".into(),
            call: RpcCall {
                method: "discoverDefinition".into(),
                params: serde_json::json!({ "repo": repo, "name": name, "source": "tui" }),
            },
            timeout_ms: 5000,
            timeout_is_ok: true,
            invalidate_defs_for: Some(repo.to_string()),
        }
    }
```

Borrow-checker note: this mirrors `run_selected_task_def`'s exact borrow pattern (copy `sel`, end the `marks` borrow before writing `status_line`). If the first `active_ui()` borrow conflicts with the bulk check + status write, hoist the bulk check into the same shape `run_or_bulk_selected_task_def` uses (check first, then resolve) — behavior identical.

`crates/qoo-tui/src/app/mouse.rs` — chip-click arm (in the `match btn` at ~line 458):

```rust
                        crate::hit::PaneButton::Discover => {
                            self.set_focus(p);
                            return self.apply_action(crate::keymap::AppAction::DiscoverSelectedDef);
                        }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p qoo-tui`
Expected: the 3 new tests PASS. Snapshot tests that render the TASKS title bar will FAIL with a new `[d]iscover` chip — review each insta diff, confirm the ONLY change is the added chip (and any chip-strip width fallout), then accept: `cargo insta accept` (or update via the repo's snapshot flow). Any diff beyond the chip is a bug.

- [ ] **Step 5: Commit**

```bash
git add crates/qoo-tui/src
git commit -m "feat(tui): explicit [d]iscover verb on the tasks pane"
```

---

### Task 3: TUI — `⌕` discovery marker in the TASKS row schedule column

**Files:**
- Modify: `crates/qoo-tui/src/selectors.rs` (new `def_sched_text` next to `def_model_text` ~line 1304; `def_col_layout` sched width calc ~line 1313)
- Modify: `crates/qoo-tui/src/view/panes.rs` (`def_line` schedule span, ~line 678-685)
- Test: `crates/qoo-tui/src/selectors.rs` (tests module)

**Interfaces:**
- Consumes: `DefinitionSummary.has_discovery`, existing `cron_human`.
- Produces: `pub fn def_sched_text(def: &DefinitionSummary) -> String` — consumed by both `def_col_layout` and `def_line` so layout width and rendered text can never desync.

- [ ] **Step 1: Write the failing tests**

In the `selectors.rs` tests module (near the `cron_human` tests, ~line 3108). Build summaries the way neighboring tests do (there are `DefinitionSummary` constructions at ~line 3161):

```rust
#[test]
fn def_sched_text_combines_cron_and_discovery_marker() {
    let mut d = crate::ipc::types::DefinitionSummary::default();
    // neither → empty
    assert_eq!(def_sched_text(&d), "");
    // discovery only → bare marker
    d.has_discovery = true;
    assert_eq!(def_sched_text(&d), "⌕");
    // cron only → humanized cron, no marker
    d.has_discovery = false;
    d.cron = Some("30 15 * * *".into());
    assert_eq!(def_sched_text(&d), "Everyday 3:30pm");
    // both → cron then marker
    d.has_discovery = true;
    assert_eq!(def_sched_text(&d), "Everyday 3:30pm ⌕");
}
```

(Confirm the exact `cron_human("30 15 * * *")` phrasing against the existing `cron_human_tier_table` test at ~line 3108 and use its literal — do not guess.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p qoo-tui def_sched_text_combines_cron_and_discovery_marker`
Expected: COMPILE ERROR (`def_sched_text` not defined).

- [ ] **Step 3: Implement**

`crates/qoo-tui/src/selectors.rs`, next to `def_model_text` (~line 1304):

```rust
/// Trailing schedule-cell text for a def row: the humanized cron schedule,
/// with a `⌕` marker appended when the def is discovery-backed (bare `⌕` for a
/// discovery-only def). Empty when the def has neither. Single source for BOTH
/// the layout width ([`def_col_layout`]) and the rendered cell
/// ([`crate::view::panes`]) so they can never desync.
pub fn def_sched_text(def: &DefinitionSummary) -> String {
    let cron = def.cron.as_deref().and_then(cron_human);
    match (cron, def.has_discovery) {
        (Some(c), true) => format!("{c} ⌕"),
        (Some(c), false) => c,
        (None, true) => "⌕".to_string(),
        (None, false) => String::new(),
    }
}
```

In `def_col_layout` (~line 1313), replace the sched width source:

```rust
    let sched_w = rows
        .iter()
        .map(|d| cw(&def_sched_text(d)))
        .max()
        .unwrap_or(0)
        .min(SCHED_CAP);
```

and update the stale comment below it (`// Trailing schedule column footprint …`): the column now carries the humanized cron and/or the `⌕` discovery marker (the old "no icon … per user request" note referred to the clock emoji; the discovery marker is a newer explicit request).

`crates/qoo-tui/src/view/panes.rs` `def_line` (~line 678-685), replace the cron-only span:

```rust
    // Schedule column: humanized cron and/or the `⌕` discovery marker (see
    // `def_sched_text` — layout and render share it). Teal/info like the args
    // column. Blank for a def with neither.
    let sched = crate::selectors::def_sched_text(def);
    if !sched.is_empty() {
        spans.push(Span::raw(gap));
        spans.push(Span::styled(pad_clip(&sched, layout.sched_w), Style::default().fg(p.info)));
    }
```

(Also update the `cron_human` import on `panes.rs:16` if it becomes unused — swap it for `def_sched_text`.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p qoo-tui`
Expected: new test PASSES. Existing `def_col_layout` tests (~line 2778) and any TASKS-pane snapshots may change where fixtures set `has_discovery: true` (the fixtures at selectors.rs:3161/3196/3249 do) — review each diff: sched column may now show/widen for `⌕`. Accept only marker-related diffs.

- [ ] **Step 5: Commit**

```bash
git add crates/qoo-tui/src/selectors.rs crates/qoo-tui/src/view/panes.rs
git commit -m "feat(tui): discovery marker in tasks-pane schedule column"
```

---

### Task 4: TUI — `discovery` definition detail sub-tab

**Files:**
- Modify: `crates/qoo-tui/src/detail.rs` (`DEF_TABS` line 25; tests at lines 150, 160)
- Modify: `crates/qoo-tui/src/view/detail.rs` (`content_for` Definition arm, ~line 391-399)
- Test: `crates/qoo-tui/src/view/detail.rs` (tests module, near `config_view_aligns_keys_and_folds_resolved_model` ~line 1405)

**Interfaces:**
- Consumes: `TaskDefinition.discovery: Option<Discovery>` with `Discovery { command: String, item_key: String }` (`ipc/types.rs:251-254`); existing `fenced` helper and `content_for` placeholder convention (empty lines + placeholder string).
- Produces: `DEF_TABS = ["prompt", "config", "discovery"]`; sub_tab 2 content for `DetailContext::Definition`.

- [ ] **Step 1: Write the failing tests**

Update `crates/qoo-tui/src/detail.rs` tests (these two lines exist today and pin the old shape):

```rust
        assert_eq!(sub_tab_names(DetailKind::Definition), &["prompt", "config", "discovery"]);
```

and the clamp test (line 160):

```rust
        assert_eq!(clamp_sub_tab(3, DetailKind::Definition), 2);
```

Add to `crates/qoo-tui/src/view/detail.rs` tests (build the `TaskDefinition` fixture the way `config_view_aligns_keys_and_folds_resolved_model` at ~line 1405 does):

```rust
#[test]
fn definition_discovery_tab_shows_command_and_item_key() {
    let mut def = crate::ipc::types::TaskDefinition::default();
    def.discovery = Some(crate::ipc::types::Discovery {
        command: "gh pr list --json url\njq '.[]'".to_string(),
        item_key: "{{url}}".to_string(),
    });
    let ctx = DetailContext::Definition { repo: "p".into(), name: "pr-review".into() };
    let (lines, _, placeholder) = content_for(&ctx, 2, Some(&def), None, 0, 0, 0);
    assert_eq!(placeholder, "");
    assert!(lines.iter().any(|l| l == "gh pr list --json url"), "lines: {lines:?}");
    assert!(lines.iter().any(|l| l == "jq '.[]'"), "multi-line command preserved");
    assert!(lines.iter().any(|l| l == "item key: {{url}}"), "lines: {lines:?}");
}

#[test]
fn definition_discovery_tab_placeholder_when_no_discovery() {
    let def = crate::ipc::types::TaskDefinition::default();
    let ctx = DetailContext::Definition { repo: "p".into(), name: "lint".into() };
    let (lines, _, placeholder) = content_for(&ctx, 2, Some(&def), None, 0, 0, 0);
    assert!(lines.is_empty());
    assert_eq!(placeholder, "(no discovery)");
}
```

(Match `content_for`'s real signature — it takes `ctx, sub_tab, def, run_files, detail_row, now_epoch_s, tz_offset_s`; copy the call shape from an existing test in this module.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p qoo-tui definition_discovery`
Expected: FAIL — `sub_tab_names` test fails on the old 2-tab list; the new tests fail because sub_tab 2 clamps to 1 (config) today.

- [ ] **Step 3: Implement**

`crates/qoo-tui/src/detail.rs` line 25:

```rust
const DEF_TABS: &[&str] = &["prompt", "config", "discovery"];
```

`crates/qoo-tui/src/view/detail.rs`, extend the Definition arm (~line 391):

```rust
        DetailContext::Definition { .. } => match def {
            None => (Vec::new(), Vec::new(), "(loading definition…)"),
            Some(d) if sub_tab == 1 => {
                let (lines, key_col) = config_view(d);
                let ctxs = vec![LineCtx::Config { key_col }; lines.len()];
                (lines, ctxs, "")
            }
            // Sub-tab 2: the full multi-line discovery command + item key
            // template. The config tab keeps its one-line `discovery` row; this
            // tab exists because real discovery commands don't fit one line.
            Some(d) if sub_tab == 2 => match &d.discovery {
                Some(disc) => {
                    let mut lines: Vec<String> =
                        disc.command.split('\n').map(str::to_string).collect();
                    if !disc.item_key.is_empty() {
                        lines.push(String::new());
                        lines.push(format!("item key: {}", disc.item_key));
                    }
                    fenced(lines, "")
                }
                None => (Vec::new(), Vec::new(), "(no discovery)"),
            },
            Some(d) => fenced(d.prompt.split('\n').map(str::to_string).collect(), "(no prompt)"),
        },
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p qoo-tui`
Expected: all PASS. Snapshots rendering the definition detail's sub-tab strip will show the third tab — review diffs (only the added `discovery` tab label), then accept. Check the test at `view/detail.rs:941` (`ui.sub_tab[DetailKind::Definition as usize] = 1`) still passes — it should, tab indices 0/1 are unchanged.

- [ ] **Step 5: Full verification + commit**

Run: `cargo test -p qoo-tui && pnpm -r test && pnpm -r typecheck && pnpm lint:ci`
Expected: all green.

```bash
git add crates/qoo-tui/src docs/superpowers/plans/2026-07-14-explicit-discover-verb.md
git commit -m "feat(tui): discovery sub-tab in definition detail"
```
