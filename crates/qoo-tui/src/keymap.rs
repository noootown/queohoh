use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::PaneId;
use crate::hit::{PaneButton, pane_buttons};

/// Pure key → action for `Mode::List`. Focus is invariantly one of the three
/// list panes (detail is display-only and never focused). The pane-action verbs
/// (`a` actions, `t` tasks, `c` create, `r` run) are GATED on the focused pane
/// actually showing that chip — `pane_buttons(focus)` is the same set the title
/// bar renders, so a key does nothing on a pane whose chip isn't there (e.g. `a`
/// is inert on TASKS, which shows only `[r]un [z]`). `z` (collapse) is on every
/// pane, so it stays effectively global. The vim keys address the DETAIL pane
/// rather than the left panes: `j`/`k` move its row cursor (or scroll it),
/// `h`/`l` cycle its sub-tab (aliasing `ctrl+x`/`ctrl+z`); the LEFT-pane cursor
/// moves with the ARROW keys (`shift` extends the contiguous range; `space`
/// toggles the cursor row's mark, which builds a NON-contiguous selection —
/// the two combine, see `view::selected_positions`). `Enter` opens the
/// selected worktree lane-task. Project-tab cycling (`CycleTab`) is driven by
/// the stateful `ctrl+s` prefix in `App`, not here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppAction {
    MoveCursor(i32),
    ExtendSelection(i32),
    /// `Space`: toggle the cursor row into the focused pane's marked set — the
    /// non-contiguous half of a bulk selection (`Shift+Arrow` covers the
    /// contiguous half). Toggle-in-place: the cursor does not move and the
    /// anchor is untouched. Live on all three list panes, since marking is a
    /// selection primitive independent of which bulk VERBS a pane supports
    /// (`hit::bulk_allowed` still governs that). Routes to `App::toggle_mark`.
    ToggleMark,
    /// `j`/`k`: move the DETAIL pane's row cursor (worktree lane-task list) when
    /// the detail shows selectable rows, else scroll the detail one line. Never
    /// dead — the App resolves which by inspecting the current detail context.
    DetailRowMove(i32),
    /// `Enter`: open the lane-task row selected in the worktree detail — jump to
    /// that task's QUEUE detail (select it in the queue pane, Run/transcript).
    /// Inert (App no-ops with a status line) when the detail has no selectable
    /// row or the task isn't in the current queue view.
    OpenDetailRow,
    CyclePane(i32),
    SwitchTab(usize),
    CycleTab(i32),
    CycleSubTab(i32),
    /// Open the task menu (`t`): the upgraded def picker over the active repo,
    /// carrying the selected worktree row's context when the worktrees pane holds
    /// focus. Routes to `App::open_task_menu`.
    OpenTaskMenu,
    /// Open the run form for the TASKS pane's highlighted definition (`r`, and
    /// the tasks pane's `[r]un` chip): always includes the effective-chain model
    /// picker (model-only form when the def has no args). Routes to
    /// `App::run_selected_task_def`.
    RunSelectedDef,
    /// Open a confirm dialog for the TASKS pane's highlighted definition's
    /// DISCOVERY (`d`, and the tasks `[d]iscover` chip). Confirm fans out one
    /// task per discovered item; cancel leaves nothing pending. Defs without a
    /// discovery block refuse with a status line (no dialog). Routes to
    /// `App::discover_selected_def`.
    DiscoverSelectedDef,
    /// Toggle the TASKS pane's highlighted definition's cron on/off (`o`, and the
    /// tasks `[o]cron` chip): pause a running schedule or resume a paused one via
    /// the `set_cron_enabled` RPC. A def with no `cron:` refuses with a status
    /// line (no RPC). Routes to `App::toggle_cron`.
    ToggleCron,
    /// Re-queue the QUEUE pane's selected task(s) (`r`, and the queue's `[r]un`
    /// chip): terminal / needs-input tasks re-run; queued/running are a no-op. A
    /// range re-queues every eligible member. Routes to `App::requeue_selected`.
    RequeueSelected,
    /// Stop the QUEUE pane's selected task(s) (`x`, and the queue's `[x]stop`
    /// chip): queued/needs-input → skip, running → stop, terminal → no-op. A
    /// range stops each eligible member. Routes to `App::cancel_selected`.
    CancelSelected,
    /// Archive TOGGLE on the QUEUE pane's selected row (`a`, and the queue's
    /// `[a]rchive` chip): an archived row restores to the live list, a terminal
    /// or parked `needs-input` row archives out of it; only active queued/running
    /// rows refuse with a status line. Routes to `App::archive_selected`.
    ArchiveSelected,
    /// New adhoc task on the selected WORKTREES row (`r`, and the worktrees
    /// `[r]un` chip): opens the session picker (`Mode::SessionPick`) for the
    /// worktree, which then leads to a launch `Mode::Form` (model + prompt).
    /// Session rows can't host a task (status line, no mode change). Routes to
    /// `App::new_task_on_worktree`.
    NewTaskOnWorktree,
    /// Open the selected WORKTREES row in a new tmux window (`g`, and the
    /// worktrees `[g]oto` chip): works for session rows too. Inert with a status
    /// line outside tmux. Routes to `App::goto_worktree`.
    GotoWorktree,
    /// Resume the QUEUE pane's selected task's Claude session in a new tmux
    /// window (`g`, and the queue `[g]oto` chip): rooted at the task's recorded
    /// worktree path. Inert with a status line outside tmux, when the task has
    /// no recorded session, or when no worktree path resolves. Routes to
    /// `App::goto_queue`.
    GotoQueue,
    /// Remove the selected WORKTREES row (`x`, and the worktrees `[x]remove`
    /// chip): opens `Mode::Confirm`. Session rows / busy worktrees are a
    /// no-op with a status line. Routes to `App::remove_selected_worktree`.
    RemoveSelectedWorktree,
    Create,
    /// Collapse/expand the focused list pane (`z`). `x` is reserved (unbound).
    ToggleCollapse,
    OpenSearch,
    ClearEsc,
    /// `Home`/`End`: scroll ONLY the detail pane to head/tail (dir < 0 = head,
    /// dir > 0 = tail). It never moves the list cursor, so Home/End are pure
    /// detail-pane controls.
    DetailScrollEdge(i32),
    SwitchSubTab(usize),
    Help,
    /// Read-only model-alias settings overlay (`s`). Distinct from the `ctrl+s`
    /// project-tab prefix, which `App` consumes before the keymap ever sees it.
    Settings,
    /// `p`: cycle the operator's ACTIVE provider to the next ENABLED one (in the
    /// settings payload's provider-precedence order, skipping disabled). Global —
    /// like the top-right `↯ <provider>` indicator it drives, it is not gated on
    /// any pane's chip. A single enabled provider (or settings not yet fetched)
    /// makes it a no-op with no RPC. Distinct from the `ctrl+s`-prefixed `p`
    /// (previous project tab), which `App` consumes before the keymap sees it.
    CycleProvider,
    Quit,
    None,
}

/// KeyEvent → AppAction in `Mode::List`. Pure. Focus is always a list pane, so
/// there is no lists-vs-detail branching. Version note: crossterm delivers
/// shifted letters as uppercase `Char('J')` with `SHIFT` set; we match on the
/// char and treat the modifier as advisory.
pub fn list_mode_action(key: &KeyEvent, focus: PaneId) -> AppAction {
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    // A pane-action verb fires only when the focused pane's title bar shows the
    // matching chip (the same set the renderer draws); otherwise it is inert.
    let gated = |btn: PaneButton, action: AppAction| {
        if pane_buttons(focus).contains(&btn) { action } else { AppAction::None }
    };
    match key.code {
        // ctrl+x / ctrl+z cycle the detail sub-tab globally (no detail focus
        // needed). Guarded before the bare `z` (collapse) arm so ctrl+z is the
        // sub-tab cycle while plain z toggles collapse.
        KeyCode::Char('x') if ctrl => AppAction::CycleSubTab(1),
        KeyCode::Char('z') if ctrl => AppAction::CycleSubTab(-1),
        KeyCode::Tab => AppAction::CyclePane(1),
        KeyCode::BackTab => AppAction::CyclePane(-1),
        KeyCode::Char(c @ '1'..='9') => AppAction::SwitchTab(c as usize - '1' as usize),
        KeyCode::Char('0') => AppAction::SwitchTab(9),
        KeyCode::Char('q') => AppAction::Quit,
        KeyCode::Char('?') => AppAction::Help,
        // Plain `s` opens the settings overlay. `ctrl+s` (the project-tab prefix)
        // never reaches here — `App` arms/consumes it before dispatching to the
        // keymap — so this bare arm can't shadow it.
        KeyCode::Char('s') => AppAction::Settings,
        // Plain `p` cycles the active provider (global, ungated — it drives the
        // always-visible top-right indicator). The `ctrl+s`-prefixed `p`
        // (previous project tab) is consumed by `App` before the keymap runs, so
        // this bare arm can't shadow it.
        KeyCode::Char('p') => AppAction::CycleProvider,
        // Pane-action verbs, each gated on the focused pane's chip set:
        // QUEUE {r,x,g,a,c,z} · TASKS {r,d,z} · WORKTREES {r,g,x,t,c,z}.
        KeyCode::Char('t') => gated(PaneButton::Tasks, AppAction::OpenTaskMenu),
        // `r` is a Run chip on all three panes, but means different things:
        // QUEUE re-queues the selected task(s), TASKS runs the highlighted def,
        // WORKTREES opens a fresh worktree-targeted new task.
        KeyCode::Char('r') => match focus {
            PaneId::Queue => gated(PaneButton::Run, AppAction::RequeueSelected),
            PaneId::Worktrees => gated(PaneButton::Run, AppAction::NewTaskOnWorktree),
            _ => gated(PaneButton::Run, AppAction::RunSelectedDef),
        },
        // `d` is a TASKS-only chip: run the highlighted def's discovery fan-out.
        KeyCode::Char('d') => gated(PaneButton::Discover, AppAction::DiscoverSelectedDef),
        // `o` is a TASKS-only chip: toggle the highlighted def's cron on/off.
        KeyCode::Char('o') => gated(PaneButton::Cron, AppAction::ToggleCron),
        // `g` is a Goto chip on QUEUE and WORKTREES, but means different things:
        // QUEUE resumes the selected task's Claude session, WORKTREES opens the
        // worktree in a fresh tmux window. Inert on TASKS (no Goto chip there).
        KeyCode::Char('g') => match focus {
            PaneId::Queue => gated(PaneButton::Goto, AppAction::GotoQueue),
            _ => gated(PaneButton::Goto, AppAction::GotoWorktree),
        },
        // `x` (plain) means stop on QUEUE (skip/stop the selected task) and
        // remove on WORKTREES (remove the selected worktree); inert on TASKS.
        // (`ctrl+x` above is the sub-tab cycle, matched first, so it never
        // reaches here.)
        KeyCode::Char('x') => match focus {
            PaneId::Worktrees => gated(PaneButton::Remove, AppAction::RemoveSelectedWorktree),
            _ => gated(PaneButton::Cancel, AppAction::CancelSelected),
        },
        // `a` is a QUEUE-only chip: archive/unarchive toggle on the selected row.
        KeyCode::Char('a') => gated(PaneButton::Archive, AppAction::ArchiveSelected),
        KeyCode::Char('c') => gated(PaneButton::Create, AppAction::Create),
        // `z` (plain) toggles collapse.
        KeyCode::Char('z') => AppAction::ToggleCollapse,
        // Home/End scroll ONLY the detail pane (head/tail) and never touch the
        // list selection. (`g` is now the worktrees goto verb; `G` stays unbound.)
        KeyCode::Home => AppAction::DetailScrollEdge(-1),
        KeyCode::End => AppAction::DetailScrollEdge(1),
        KeyCode::Esc => AppAction::ClearEsc,
        KeyCode::Char('/') => AppAction::OpenSearch,
        // Space marks/unmarks the cursor row (non-contiguous bulk selection).
        // Ungated: every list pane can build a selection; whether a VERB may act
        // on a bulk selection is `hit::bulk_allowed`'s call, not the keymap's.
        KeyCode::Char(' ') => AppAction::ToggleMark,
        // Arrow keys drive the LEFT pane cursor (shift = extend selection). The
        // vim keys split off to the DETAIL pane: j/k move the detail row cursor
        // (or scroll), h/l cycle the detail sub-tab (aliasing ctrl+x/ctrl+z).
        KeyCode::Down => {
            if shift { AppAction::ExtendSelection(1) } else { AppAction::MoveCursor(1) }
        }
        KeyCode::Up => {
            if shift { AppAction::ExtendSelection(-1) } else { AppAction::MoveCursor(-1) }
        }
        KeyCode::Char('j') => AppAction::DetailRowMove(1),
        KeyCode::Char('k') => AppAction::DetailRowMove(-1),
        KeyCode::Char('h') => AppAction::CycleSubTab(-1),
        KeyCode::Char('l') => AppAction::CycleSubTab(1),
        // Enter opens the lane-task row selected in the worktree detail (inert
        // elsewhere — the App resolves it against the current detail context).
        KeyCode::Enter => AppAction::OpenDetailRow,
        _ => AppAction::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::PaneId;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn k(code: KeyCode) -> KeyEvent { KeyEvent::new(code, KeyModifiers::NONE) }
    fn sk(code: KeyCode) -> KeyEvent { KeyEvent::new(code, KeyModifiers::SHIFT) }
    fn ck(code: KeyCode) -> KeyEvent { KeyEvent::new(code, KeyModifiers::CONTROL) }
    // Focus is invariantly a list pane; Queue stands in for "any focus".
    const LISTS: [PaneId; 3] = [PaneId::Queue, PaneId::Tasks, PaneId::Worktrees];
    const F: PaneId = PaneId::Queue;

    #[test]
    fn q_quits() {
        assert_eq!(list_mode_action(&k(KeyCode::Char('q')), F), AppAction::Quit);
    }

    #[test]
    fn s_opens_settings() {
        // Plain `s` → Settings on any focused list pane. The keymap is
        // modifier-agnostic on the char; `App` arms/consumes the `ctrl+s`
        // project-tab prefix BEFORE dispatching to the keymap, so the ctrl case
        // never reaches this arm in practice.
        for f in LISTS {
            assert_eq!(list_mode_action(&k(KeyCode::Char('s')), f), AppAction::Settings);
        }
    }

    #[test]
    fn digits_switch_project_tabs_globally() {
        for f in LISTS {
            for n in 1..=9u32 {
                let c = char::from_digit(n, 10).unwrap();
                assert_eq!(
                    list_mode_action(&k(KeyCode::Char(c)), f),
                    AppAction::SwitchTab((n - 1) as usize)
                );
            }
            // 0 selects the 10th tab.
            assert_eq!(list_mode_action(&k(KeyCode::Char('0')), f), AppAction::SwitchTab(9));
        }
    }

    #[test]
    fn ctrl_x_z_cycle_detail_sub_tabs_globally() {
        for f in LISTS {
            assert_eq!(list_mode_action(&ck(KeyCode::Char('x')), f), AppAction::CycleSubTab(1));
            assert_eq!(list_mode_action(&ck(KeyCode::Char('z')), f), AppAction::CycleSubTab(-1));
        }
    }

    #[test]
    fn old_bracket_and_brace_bindings_are_gone() {
        for c in ['[', ']', '{', '}'] {
            assert_eq!(list_mode_action(&k(KeyCode::Char(c)), F), AppAction::None);
        }
    }

    #[test]
    fn tab_cycles_panes() {
        assert_eq!(list_mode_action(&k(KeyCode::Tab), F), AppAction::CyclePane(1));
        assert_eq!(list_mode_action(&k(KeyCode::BackTab), F), AppAction::CyclePane(-1));
    }

    #[test]
    fn arrows_move_the_left_cursor() {
        // Only the ARROW keys move the left-pane cursor now (j/k address detail).
        for f in LISTS {
            assert_eq!(list_mode_action(&k(KeyCode::Down), f), AppAction::MoveCursor(1));
            assert_eq!(list_mode_action(&k(KeyCode::Up), f), AppAction::MoveCursor(-1));
        }
    }

    #[test]
    fn jk_move_the_detail_row_not_the_left_cursor() {
        // j/k no longer touch the list cursor — they drive the detail row cursor
        // (or scroll), resolved by the App against the current detail context.
        for f in LISTS {
            assert_eq!(list_mode_action(&k(KeyCode::Char('j')), f), AppAction::DetailRowMove(1));
            assert_eq!(list_mode_action(&k(KeyCode::Char('k')), f), AppAction::DetailRowMove(-1));
        }
    }

    #[test]
    fn shift_arrows_extend_but_shift_jk_are_gone() {
        for f in LISTS {
            assert_eq!(list_mode_action(&sk(KeyCode::Down), f), AppAction::ExtendSelection(1));
            assert_eq!(list_mode_action(&sk(KeyCode::Up), f), AppAction::ExtendSelection(-1));
            // J/K no longer extend the selection (arrows own extend now).
            assert_eq!(list_mode_action(&sk(KeyCode::Char('J')), f), AppAction::None);
            assert_eq!(list_mode_action(&sk(KeyCode::Char('K')), f), AppAction::None);
        }
    }

    #[test]
    fn hl_cycle_detail_sub_tabs_and_left_right_stay_inert() {
        for f in LISTS {
            // h/l cycle the detail sub-tab (aliasing ctrl+z/ctrl+x).
            assert_eq!(list_mode_action(&k(KeyCode::Char('l')), f), AppAction::CycleSubTab(1));
            assert_eq!(list_mode_action(&k(KeyCode::Char('h')), f), AppAction::CycleSubTab(-1));
            // Arrow Left/Right remain unbound (no horizontal focus nav).
            assert_eq!(list_mode_action(&k(KeyCode::Right), f), AppAction::None);
            assert_eq!(list_mode_action(&k(KeyCode::Left), f), AppAction::None);
        }
    }

    #[test]
    fn enter_opens_the_selected_detail_row() {
        // Enter now opens the selected worktree lane-task (the App no-ops it on
        // other detail contexts).
        for f in LISTS {
            assert_eq!(list_mode_action(&k(KeyCode::Enter), f), AppAction::OpenDetailRow);
        }
    }

    #[test]
    fn c_gated_by_pane_slash_esc_help_global() {
        // `?`/esc/`/` are global on every list pane.
        for f in LISTS {
            assert_eq!(list_mode_action(&k(KeyCode::Char('?')), f), AppAction::Help);
            assert_eq!(list_mode_action(&k(KeyCode::Esc), f), AppAction::ClearEsc);
            assert_eq!(list_mode_action(&k(KeyCode::Char('/')), f), AppAction::OpenSearch);
        }
        // `a` is the QUEUE archive/unarchive toggle (the old queue `[a]ctions`
        // menu was retired in favor of `[g]oto` long before); inert on the other
        // panes (no `[a]rchive` chip there).
        assert_eq!(
            list_mode_action(&k(KeyCode::Char('a')), PaneId::Queue),
            AppAction::ArchiveSelected
        );
        assert_eq!(list_mode_action(&k(KeyCode::Char('a')), PaneId::Tasks), AppAction::None);
        assert_eq!(list_mode_action(&k(KeyCode::Char('a')), PaneId::Worktrees), AppAction::None);
        // `c` (create) is a `[c]reate` chip on EVERY list pane now — the unified
        // adhoc create form, prefilled per pane.
        assert_eq!(list_mode_action(&k(KeyCode::Char('c')), PaneId::Queue), AppAction::Create);
        assert_eq!(list_mode_action(&k(KeyCode::Char('c')), PaneId::Worktrees), AppAction::Create);
        assert_eq!(list_mode_action(&k(KeyCode::Char('c')), PaneId::Tasks), AppAction::Create);
    }

    #[test]
    fn g_gotos_on_queue_and_worktrees_shift_g_unbound() {
        // `g` is a Goto chip on QUEUE (resume the task's Claude session) and
        // WORKTREES (open the worktree in tmux); inert on TASKS (no `[g]oto`
        // chip there).
        assert_eq!(list_mode_action(&k(KeyCode::Char('g')), PaneId::Queue), AppAction::GotoQueue);
        assert_eq!(list_mode_action(&k(KeyCode::Char('g')), PaneId::Worktrees), AppAction::GotoWorktree);
        assert_eq!(list_mode_action(&k(KeyCode::Char('g')), PaneId::Tasks), AppAction::None);
        // Shift+G stays unbound everywhere.
        for f in LISTS {
            assert_eq!(list_mode_action(&sk(KeyCode::Char('G')), f), AppAction::None);
        }
    }

    #[test]
    fn worktree_pane_r_g_x_verbs() {
        // The three worktrees row verbs: `r` opens the worktree's session
        // launcher, `g` gotos (tmux), `x` removes.
        assert_eq!(list_mode_action(&k(KeyCode::Char('r')), PaneId::Worktrees), AppAction::NewTaskOnWorktree);
        assert_eq!(list_mode_action(&k(KeyCode::Char('g')), PaneId::Worktrees), AppAction::GotoWorktree);
        assert_eq!(list_mode_action(&k(KeyCode::Char('x')), PaneId::Worktrees), AppAction::RemoveSelectedWorktree);
        // g resumes the session on queue (not inert); x still cancels on queue;
        // a inert on worktrees now.
        assert_eq!(list_mode_action(&k(KeyCode::Char('g')), PaneId::Queue), AppAction::GotoQueue);
        assert_eq!(list_mode_action(&k(KeyCode::Char('x')), PaneId::Queue), AppAction::CancelSelected);
        assert_eq!(list_mode_action(&k(KeyCode::Char('a')), PaneId::Worktrees), AppAction::None);
    }

    #[test]
    fn home_end_edges() {
        // Home/End are detail-only: they emit DetailScrollEdge (head/tail) on
        // every focused list pane and never move the left-side list cursor.
        for f in LISTS {
            assert_eq!(list_mode_action(&k(KeyCode::Home), f), AppAction::DetailScrollEdge(-1));
            assert_eq!(list_mode_action(&k(KeyCode::End), f), AppAction::DetailScrollEdge(1));
        }
    }

    #[test]
    fn z_toggles_collapse_but_ctrl_z_does_not() {
        // Plain z toggles collapse; ctrl+z is the sub-tab cycle. The two coexist.
        assert_eq!(list_mode_action(&k(KeyCode::Char('z')), F), AppAction::ToggleCollapse);
        assert_eq!(list_mode_action(&ck(KeyCode::Char('z')), F), AppAction::CycleSubTab(-1));
    }

    #[test]
    fn x_stops_on_queue_removes_on_worktrees_ctrl_x_still_cycles() {
        // Plain `x`: stop on QUEUE (its `[x]stop` chip), remove on WORKTREES
        // (its `[x]remove` chip); inert on TASKS (no `x` chip there).
        assert_eq!(list_mode_action(&k(KeyCode::Char('x')), PaneId::Queue), AppAction::CancelSelected);
        assert_eq!(list_mode_action(&k(KeyCode::Char('x')), PaneId::Worktrees), AppAction::RemoveSelectedWorktree);
        assert_eq!(list_mode_action(&k(KeyCode::Char('x')), PaneId::Tasks), AppAction::None);
        // ctrl+x remains the forward sub-tab cycle (matched before plain `x`).
        assert_eq!(list_mode_action(&ck(KeyCode::Char('x')), F), AppAction::CycleSubTab(1));
    }

    #[test]
    fn unbound_keys_are_none() {
        // w/f/m moved to the action menu (parity with the Ink keymap tests). On
        // the QUEUE pane `t` is inert (a WORKTREES-only chip, keymap-gated).
        // `r`/`x`/`g` ARE bound on QUEUE now (re-queue / cancel / goto), so
        // they're not in this set.
        for c in ['w', 'f', 'm', 't'] {
            assert_eq!(list_mode_action(&k(KeyCode::Char(c)), PaneId::Queue), AppAction::None);
        }
    }

    #[test]
    fn r_runs_def_on_tasks_requeues_on_queue_new_task_on_worktrees() {
        // `r` is a Run chip on all three panes, meaning different verbs: TASKS
        // runs the highlighted def; QUEUE re-queues the selected task(s);
        // WORKTREES opens the worktree's session launcher.
        assert_eq!(list_mode_action(&k(KeyCode::Char('r')), PaneId::Tasks), AppAction::RunSelectedDef);
        assert_eq!(list_mode_action(&k(KeyCode::Char('r')), PaneId::Queue), AppAction::RequeueSelected);
        assert_eq!(list_mode_action(&k(KeyCode::Char('r')), PaneId::Worktrees), AppAction::NewTaskOnWorktree);
    }

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

    #[test]
    fn o_toggles_cron_on_tasks_only() {
        assert_eq!(
            list_mode_action(&k(KeyCode::Char('o')), PaneId::Tasks),
            AppAction::ToggleCron
        );
        // No Cron chip on QUEUE / WORKTREES → the gate leaves `o` inert there.
        assert_eq!(list_mode_action(&k(KeyCode::Char('o')), PaneId::Queue), AppAction::None);
        assert_eq!(list_mode_action(&k(KeyCode::Char('o')), PaneId::Worktrees), AppAction::None);
    }

    #[test]
    fn space_toggles_a_mark_on_every_list_pane() {
        // Ungated: marking is a selection primitive, live on all three panes.
        for f in LISTS {
            assert_eq!(list_mode_action(&k(KeyCode::Char(' ')), f), AppAction::ToggleMark);
        }
    }

    #[test]
    fn p_cycles_provider_globally() {
        // Plain `p` cycles the active provider on every focused list pane
        // (ungated, like the indicator it drives). The `ctrl+s`-prefixed `p`
        // (previous tab) never reaches the keymap — `App` consumes it first.
        for f in LISTS {
            assert_eq!(list_mode_action(&k(KeyCode::Char('p')), f), AppAction::CycleProvider);
        }
    }

    #[test]
    fn t_opens_task_menu_only_on_worktrees() {
        // `t` is a WORKTREES chip: opens the task menu there, inert on
        // queue/tasks.
        assert_eq!(list_mode_action(&k(KeyCode::Char('t')), PaneId::Worktrees), AppAction::OpenTaskMenu);
        for f in [PaneId::Queue, PaneId::Tasks] {
            assert_eq!(list_mode_action(&k(KeyCode::Char('t')), f), AppAction::None);
        }
    }
}
