//! Top-level event dispatch for `App`.
//!
//! `update` is the single entry point the event loop calls; `update_event`
//! fans an [`Event`] out to the mode-specific key/mouse handlers and snapshot
//! wiring. Split out of `app/mod.rs` verbatim (no behavior change).

use super::*;

impl App {
    pub fn update(&mut self, event: Event) -> Update {
        let up = self.update_event(event);
        // After any event, realign the live layout fields to the active project.
        // A tab switch or the first snapshot changes the active project; this is
        // the single place the stash-old / load-new swap happens. The event that
        // moves the active project (snapshot, tab switch) is already `dirty`, so
        // the swap needs no extra redraw signal of its own.
        self.reconcile_active_layout();
        up
    }

    fn update_event(&mut self, event: Event) -> Update {
        match event {
            Event::Snapshot(snapshot) => {
                self.snapshot = Some(snapshot);
                self.connected = true;
                let mut cmds = Vec::new();
                // A fresh snapshot can change (or first-establish) the selected
                // run — debounce a tail read for it.
                self.schedule_run_read(&mut cmds, 120);
                // Daemon self-heal: compare the reported build to disk and act
                // (Defer/RestartNow status + a Cmd::Heal on restart-now).
                cmds.extend(self.heal_on_snapshot());
                // Fetch the settings payload once on connect so the provider
                // switch (`p` / the ↯ indicator click) has the enabled-providers
                // list to cycle over — the always-visible indicator itself reads
                // the snapshot's `active_provider`, but cycling needs the ordered
                // provider set, which lives only in the settings payload. Same
                // `is_none()` guard the `s` overlay uses (a cached Some(None)
                // failure never re-fetches); mirrors the lazy `reconcile_defs`
                // pattern.
                if self.settings.is_none() {
                    cmds.push(Cmd::FetchSettings);
                }
                Update { dirty: true, cmds }
            }
            Event::RunFiles { task_id, files } => {
                let mut cmds = Vec::new();
                // Stale-read discard: the selection moved while the read was in
                // flight.
                let Some((sel_id, running)) = self.selected_run_task() else {
                    return Update { dirty: false, cmds };
                };
                if task_id != sel_id {
                    return Update { dirty: false, cmds };
                }
                // Poll loop via events: while the selected task runs, each read
                // result arms the next 1s read — no timer state in App.
                if running {
                    cmds.push(Cmd::ReadRunFiles {
                        task_id: sel_id.clone(),
                        tail_lines: self.tail_lines(),
                        delay_ms: 1000,
                    });
                }
                // Identical-content skip: quiet poll → 0 renders.
                let identical = self
                    .run_files
                    .as_ref()
                    .map(|(id, f)| *id == task_id && *f == files)
                    .unwrap_or(false);
                if identical {
                    return Update { dirty: false, cmds };
                }
                self.run_files = Some((task_id, files));
                Update { dirty: true, cmds }
            }
            Event::Disconnected => {
                // The retry loop re-sends this every ~2s while the daemon is
                // down; only the transition repaints (zero idle renders).
                let was_connected = self.connected;
                self.connected = false;
                Update { dirty: was_connected, cmds: vec![] }
            }
            Event::Resize => Update { dirty: true, cmds: vec![] },
            Event::Tick => {
                self.now_epoch_s = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                // Zero idle renders: the elapsed-label repaint only matters while
                // the active project has a running task. The main loop also gates
                // the Tick arm on `wants_tick`; this is the defensive second layer.
                Update { dirty: self.wants_tick(), cmds: vec![] }
            }
            // These overlay modes swallow keys (checked before generic list
            // handling). Guards include the Press filter so key-release events
            // fall through to the generic arm's no-op.
            // Unified destructive-confirm. Left/Right/Tab move focus between the
            // two buttons; Enter activates the FOCUSED one; `y`/`n` are always-on
            // accelerators; Esc dismisses (unadvertised — the button row is the
            // only hint).
            Event::Key(k)
                if k.kind == KeyEventKind::Press && matches!(self.mode, Mode::Confirm { .. }) =>
            {
                use crate::hit::ButtonKind;
                use crossterm::event::KeyCode::*;
                match k.code {
                    Left | Right | Tab | BackTab => {
                        if let Mode::Confirm { focus, .. } = &mut self.mode {
                            *focus = match *focus {
                                ButtonKind::Confirm => ButtonKind::Cancel,
                                ButtonKind::Cancel => ButtonKind::Confirm,
                            };
                        }
                        Update { dirty: true, cmds: vec![] }
                    }
                    Char('y') => self.confirm_dialog_fire(),
                    Enter if matches!(self.mode, Mode::Confirm { focus: ButtonKind::Confirm, .. }) => {
                        self.confirm_dialog_fire()
                    }
                    // Enter-on-Cancel joins the dismiss accelerators.
                    Char('n') | Char('q') | Esc | Enter => {
                        self.mode = Mode::List;
                        Update { dirty: true, cmds: vec![] }
                    }
                    _ => Update { dirty: false, cmds: vec![] },
                }
            }
            // Def-pick popup owns keys while open (checked before generic list
            // handling); key-release falls through to the generic no-op.
            Event::Key(k)
                if k.kind == KeyEventKind::Press && matches!(self.mode, Mode::DefPick { .. }) =>
            {
                self.def_pick_key(&k)
            }
            // Args form owns keys while open (checked before generic list
            // handling); key-release falls through to the generic no-op.
            Event::Key(k)
                if k.kind == KeyEventKind::Press && matches!(self.mode, Mode::DefArgs { .. }) =>
            {
                self.def_args_key(&k)
            }
            // Session picker owns keys while open (checked before generic list
            // handling); key-release falls through to the generic no-op.
            Event::Key(k)
                if k.kind == KeyEventKind::Press && matches!(self.mode, Mode::SessionPick { .. }) =>
            {
                self.session_pick_key(&k)
            }
            // The bordered form owns keys while open (checked before generic list
            // handling); key-release falls through to the generic no-op.
            Event::Key(k)
                if k.kind == KeyEventKind::Press && matches!(self.mode, Mode::Form { .. }) =>
            {
                self.form_key(&k)
            }
            Event::Key(key) => {
                if key.kind != KeyEventKind::Press {
                    return Update { dirty: false, cmds: vec![] };
                }
                match &self.mode {
                    Mode::Help | Mode::Settings => {
                        // Any key closes the help / settings overlay.
                        self.mode = Mode::List;
                        Update { dirty: true, cmds: Vec::new() }
                    }
                    Mode::Search { pane } => {
                        let pane = *pane;
                        let mut dirty = true;
                        // Keystrokes that reset the cursor to 0 (printable,
                        // backspace), Enter-apply, and Esc-clear all change the
                        // effective selection, so they must schedule the debounced
                        // run-file read like every other selection path.
                        let mut cmds = Vec::new();
                        match key.code {
                            KeyCode::Enter => {
                                self.mode = Mode::List; // apply
                                self.schedule_run_read(&mut cmds, 120);
                            }
                            KeyCode::Esc => {
                                self.ui().search[pane as usize].clear();
                                self.ui().selections[pane as usize] =
                                    Selection { cursor: 0, anchor: None };
                                self.mode = Mode::List;
                                self.schedule_run_read(&mut cmds, 120);
                            }
                            KeyCode::Backspace => {
                                self.ui().search[pane as usize].pop();
                                self.ui().selections[pane as usize] =
                                    Selection { cursor: 0, anchor: None };
                                self.schedule_run_read(&mut cmds, 120);
                            }
                            KeyCode::Char(c)
                                if !key.modifiers.contains(
                                    crossterm::event::KeyModifiers::CONTROL,
                                ) =>
                            {
                                self.ui().search[pane as usize].push(c);
                                self.ui().selections[pane as usize] =
                                    Selection { cursor: 0, anchor: None };
                                self.schedule_run_read(&mut cmds, 120);
                            }
                            _ => dirty = false,
                        }
                        Update { dirty, cmds }
                    }
                    Mode::List => {
                        // Status line clears on ANY list-mode keypress (even unbound keys).
                        let had_status = self.status_line.take().is_some();
                        // tmux-style prefix: when armed, this key is consumed —
                        // `n`/`p` cycle project tabs (wrapping), anything else just
                        // disarms and is swallowed. Disarming always repaints (the
                        // footer indicator turns off).
                        if self.prefix_armed {
                            self.prefix_armed = false;
                            let action = match key.code {
                                KeyCode::Char('n') => crate::keymap::AppAction::CycleTab(1),
                                KeyCode::Char('p') => crate::keymap::AppAction::CycleTab(-1),
                                _ => crate::keymap::AppAction::None,
                            };
                            let up = self.apply_action(action);
                            return Update { dirty: true, cmds: up.cmds };
                        }
                        // Arm the prefix on ctrl+s. Consumed here so it never
                        // reaches the keymap; the next key resolves it above.
                        if matches!(key.code, KeyCode::Char('s'))
                            && key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                        {
                            self.prefix_armed = true;
                            return Update { dirty: true, cmds: Vec::new() };
                        }
                        let action = crate::keymap::list_mode_action(&key, self.ui().focus);
                        let mut up = self.apply_action(action);
                        up.dirty = up.dirty || had_status;
                        up
                    }
                    // The overlay modes (Confirm, DefPick, DefArgs, SessionPick,
                    // Form) only reach here on key-release — their Press events
                    // are handled by the guarded arms above.
                    _ => Update { dirty: false, cmds: vec![] },
                }
            }
            // Bracketed paste. Every text target sanitizes the payload
            // (`sanitize_paste`: CR/CRLF → newline — terminals send CR for
            // line breaks — tabs expanded, other control chars dropped so the
            // wrap math matches what the renderer can draw). A textarea keeps
            // the line structure (the "paste your bug report" flow); the
            // single-line prompt/worktree/branch inputs flatten line breaks to
            // spaces so a multiline paste can't smuggle a newline into a
            // one-line field. Every other mode ignores paste (List has no text
            // target).
            Event::Paste(s) => match &mut self.mode {
                Mode::DefArgs { state, .. } => {
                    state.insert_str(&s);
                    Update { dirty: !s.is_empty(), cmds: vec![] }
                }
                // The form routes paste to its focused text field (textarea
                // keeps line structure; single-line input flattens it).
                Mode::Form { state, .. } => {
                    state.insert_str(&s);
                    Update { dirty: !s.is_empty(), cmds: vec![] }
                }
                _ => Update { dirty: false, cmds: vec![] },
            },
            Event::Mouse(m) => self.on_mouse(m),
            Event::ActionResult { status, invalidate_defs_for } => {
                // Success carries status = None → leave the line untouched (never clobber
                // a heal/create message with an empty). Failure carries the message.
                if status.is_some() {
                    self.status_line = status;
                }
                // A self-heal reported its outcome (success emits nothing; failure carries
                // "daemon busy — restart deferred"). Clear the in-flight flag and mark the
                // status heal-owned so the next healthy snapshot clears it.
                if self.healing {
                    self.healing = false;
                    self.heal_status_shown = true;
                }
                let mut cmds = Vec::new();
                if let Some(repo) = invalidate_defs_for {
                    // A run may change dedup state, so drop the cached defs and re-fetch
                    // eagerly (ports App.tsx `invalidateDefs` + the lazy re-fetch effect).
                    // Mark in flight so the event loop's `reconcile_defs` dedups against
                    // this eager re-fetch instead of emitting a duplicate.
                    self.defs_by_project.remove(&repo);
                    self.defs_inflight.insert(repo.clone());
                    // Full definitions may be stale for the same reason; dropping
                    // them (and their poison markers) lets `reconcile_full_def`
                    // lazily refetch whichever one the detail pane shows next.
                    let prefix = format!("{repo}/");
                    self.full_defs.retain(|k, _| !k.starts_with(&prefix));
                    self.full_defs_inflight.retain(|k| !k.starts_with(&prefix));
                    cmds.push(Cmd::FetchDefinitions { repo });
                }
                Update { dirty: true, cmds }
            }
            Event::Definitions { repo, defs } => {
                // Cache the repo's summaries and clear its in-flight flag so the
                // next `reconcile_defs` sees it cached (ports the TS effect that
                // stores the fetch result and re-enables re-fetch after invalidation).
                // The daemon's `definitions` call returns entries for EVERY project
                // (a global def appears once per project) — keep only this repo's
                // (ports App.tsx `all.filter((d) => d.repo === activeName)`).
                let defs: Vec<DefinitionSummary> =
                    defs.into_iter().filter(|d| d.repo == repo).collect();
                self.defs_by_project.insert(repo.clone(), defs);
                self.defs_inflight.remove(&repo);
                // A landed refetch means any `d`-discover on this repo has
                // resolved (the discover RPC's ActionResult triggers exactly
                // this refetch, on success AND on error/timeout) — stop the
                // def rows' `⌕`-spinner.
                let prefix = format!("{repo}/");
                self.discovering.retain(|k| !k.starts_with(&prefix));
                Update { dirty: true, cmds: vec![] }
            }
            Event::Settings { payload } => {
                // Store the fetch outcome (payload may be None = failed/unsupported
                // → cached as Some(None) so the overlay stops "loading" and never
                // re-fetches). Repaints so an open overlay swaps from the loading
                // line to the table.
                self.settings = Some(payload);
                Update { dirty: true, cmds: vec![] }
            }
            Event::SelectionExpired { epoch } => {
                // Post-copy fade. Only the CURRENT selection generation may be
                // cleared — a stale timer racing a newer selection is a no-op.
                let clear = epoch == self.selection_epoch && self.detail_selection.is_some();
                if clear {
                    self.detail_selection = None;
                }
                Update { dirty: clear, cmds: vec![] }
            }
            Event::SessionsLoaded { worktree, result } => {
                // Only applies to a session picker still open for the SAME
                // worktree — a reply that arrives after the picker moved on (or
                // closed) is stale and dropped. Both outcomes clear `loading`.
                let fresh = matches!(
                    &self.mode,
                    Mode::SessionPick { worktree: wt, .. } if *wt == worktree
                );
                if !fresh {
                    return Update { dirty: false, cmds: vec![] };
                }
                match result {
                    Ok(v) => {
                        if let Mode::SessionPick { items, loading, .. } = &mut self.mode {
                            *items = v;
                            *loading = false;
                        }
                    }
                    Err(e) => {
                        // Keep the modal usable ("New session" still selectable);
                        // surface the error on the status line.
                        if let Mode::SessionPick { loading, .. } = &mut self.mode {
                            *loading = false;
                        }
                        self.status_line = Some(format!("list sessions: {e}"));
                    }
                }
                Update { dirty: true, cmds: vec![] }
            }
            Event::Definition { repo, name, def } => {
                // Full-definition reply for the detail pane. Success fills the
                // cache (and clears the in-flight marker); failure LEAVES the
                // marker as a poison so the per-event `reconcile_full_def` doesn't
                // refetch-loop against a broken daemon — invalidation clears it.
                let key = format!("{repo}/{name}");
                match def {
                    Some(d) => {
                        self.full_defs.insert(key.clone(), *d);
                        self.full_defs_inflight.remove(&key);
                        Update { dirty: true, cmds: vec![] }
                    }
                    None => Update { dirty: false, cmds: vec![] },
                }
            }
        }
    }

    /// Fire the open confirm dialog's frozen action and close it. Shared by the
    /// keyboard confirm path and the Confirm-button click (which acts regardless
    /// of button focus, so it can't route through the focus-dependent Enter).
    pub(super) fn confirm_dialog_fire(&mut self) -> Update {
        let action = if let Mode::Confirm { action, .. } = &self.mode {
            action.clone()
        } else {
            unreachable!()
        };
        self.mode = Mode::List;
        let cmds = self.run_confirm_action(action);
        Update { dirty: true, cmds }
    }

    /// Dispatch a confirmed [`ConfirmAction`]. Each arm produces exactly the
    /// `Cmd`s its former dedicated confirm mode produced (single `Cmd::Rpc` for a
    /// remove; a range-clearing `RpcSeq` for bulk remove, cancel, and re-queue).
    fn run_confirm_action(&mut self, action: ConfirmAction) -> Vec<Cmd> {
        match action {
            ConfirmAction::RemoveWorktree { repo, worktree } => vec![self.dispatch_rpc(
                "remove worktree",
                "removeWorktree",
                serde_json::json!({ "repo": repo, "name": worktree }),
                RpcOpts::default(),
            )],
            ConfirmAction::BulkRemoveWorktrees { repo, names } => {
                self.clear_range_and_marks(ListPane::Worktrees);
                vec![Cmd::RpcSeq {
                    verb: "removed".into(),
                    calls: names
                        .into_iter()
                        .map(|name| RpcCall {
                            method: "removeWorktree".into(),
                            params: serde_json::json!({ "repo": repo, "name": name }),
                        })
                        .collect(),
                    invalidate_defs_for: None,
                }]
            }
            ConfirmAction::CancelTasks { calls } => {
                self.clear_range_and_marks(ListPane::Queue);
                vec![Cmd::RpcSeq { verb: "cancelled".into(), calls, invalidate_defs_for: None }]
            }
            ConfirmAction::RequeueTasks { calls } => {
                self.clear_range_and_marks(ListPane::Queue);
                vec![Cmd::RpcSeq { verb: "reran".into(), calls, invalidate_defs_for: None }]
            }
            ConfirmAction::SwitchProvider { target } => {
                // Optimistic: write the new value into BOTH the live snapshot (the
                // indicator's reconcile source, so it flips instantly) and the
                // cached settings payload (so the `s` overlay agrees). The daemon's
                // next state broadcast overwrites the snapshot field authoritatively.
                if let Some(snap) = self.snapshot.as_mut() {
                    snap.active_provider = Some(target.clone());
                }
                if let Some(Some(p)) = self.settings.as_mut() {
                    p.active_provider = target.clone();
                }
                vec![self.dispatch_rpc(
                    "switch provider",
                    "set_active_provider",
                    serde_json::json!({ "provider": target }),
                    RpcOpts::default(),
                )]
            }
        }
    }
}
