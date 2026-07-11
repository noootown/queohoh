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
/// moves with the ARROW keys (`shift` extends). `Enter` opens the selected
/// worktree lane-task. Project-tab cycling (`CycleTab`) is driven by the stateful
/// `ctrl+s` prefix in `App`, not here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppAction {
    MoveCursor(i32),
    ExtendSelection(i32),
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
    OpenActionMenu,
    /// Open the task menu (`t`): the upgraded def picker over the active repo,
    /// carrying the selected worktree row's context when the worktrees pane holds
    /// focus. Routes to `App::open_task_menu`.
    OpenTaskMenu,
    /// Run the TASKS pane's highlighted definition directly (`r`, and the tasks
    /// pane's `[r]un` chip): a zero-arg def dispatches immediately, a def with
    /// args opens the run form. Routes to `App::run_selected_task_def`.
    RunSelectedDef,
    /// Re-queue the QUEUE pane's selected task(s) (`r`, and the queue's `[r]un`
    /// chip): terminal / needs-input tasks re-run; queued/running are a no-op. A
    /// range re-queues every eligible member. Routes to `App::requeue_selected`.
    RequeueSelected,
    /// Cancel the QUEUE pane's selected task(s) (`x`, and the queue's `[x] cancel`
    /// chip): queued/needs-input → skip, running → stop, terminal → no-op. A
    /// range cancels each eligible member. Routes to `App::cancel_selected`.
    CancelSelected,
    Create,
    /// Collapse/expand the focused list pane (`z`). `x` is reserved (unbound).
    ToggleCollapse,
    OpenSearch,
    ClearEsc,
    /// `g`/`G`: jump the focused list cursor to the first/last row (dir < 0 =
    /// top, dir > 0 = bottom). In `Mode::List` focus is always a list pane, so
    /// this always moves the left-side selection.
    ScrollEdge(i32),
    /// `Home`/`End`: scroll ONLY the detail pane to head/tail (dir < 0 = head,
    /// dir > 0 = tail). Deliberately distinct from `ScrollEdge`: it never moves
    /// the list cursor, so Home/End are pure detail-pane controls.
    DetailScrollEdge(i32),
    SwitchSubTab(usize),
    Help,
    /// Read-only model-alias settings overlay (`s`). Distinct from the `ctrl+s`
    /// project-tab prefix, which `App` consumes before the keymap ever sees it.
    Settings,
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
        // Pane-action verbs, each gated on the focused pane's chip set:
        // QUEUE {r,x,a,c,z} · TASKS {r,z} · WORKTREES {t,a,c,z}.
        KeyCode::Char('a') => gated(PaneButton::Actions, AppAction::OpenActionMenu),
        KeyCode::Char('t') => gated(PaneButton::Tasks, AppAction::OpenTaskMenu),
        // `r` is a Run chip on both QUEUE and TASKS, but means different things:
        // QUEUE re-queues the selected task(s), TASKS runs the highlighted def.
        KeyCode::Char('r') => match focus {
            PaneId::Queue => gated(PaneButton::Run, AppAction::RequeueSelected),
            _ => gated(PaneButton::Run, AppAction::RunSelectedDef),
        },
        // `x` (plain) cancels the selected queue task(s) — skip/stop by status;
        // a QUEUE-only chip, inert elsewhere. (`ctrl+x` above is the sub-tab
        // cycle, matched first, so it never reaches here.)
        KeyCode::Char('x') => gated(PaneButton::Cancel, AppAction::CancelSelected),
        KeyCode::Char('c') => gated(PaneButton::Create, AppAction::Create),
        // `z` (plain) toggles collapse.
        KeyCode::Char('z') => AppAction::ToggleCollapse,
        // g/G jump the list cursor to the first/last row; Home/End scroll ONLY
        // the detail pane (head/tail) and never touch the list selection.
        KeyCode::Char('g') => AppAction::ScrollEdge(-1),
        KeyCode::Char('G') => AppAction::ScrollEdge(1),
        KeyCode::Home => AppAction::DetailScrollEdge(-1),
        KeyCode::End => AppAction::DetailScrollEdge(1),
        KeyCode::Esc => AppAction::ClearEsc,
        KeyCode::Char('/') => AppAction::OpenSearch,
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
    fn a_c_gated_by_pane_slash_esc_help_global() {
        // `?`/esc/`/` are global on every list pane.
        for f in LISTS {
            assert_eq!(list_mode_action(&k(KeyCode::Char('?')), f), AppAction::Help);
            assert_eq!(list_mode_action(&k(KeyCode::Esc), f), AppAction::ClearEsc);
            assert_eq!(list_mode_action(&k(KeyCode::Char('/')), f), AppAction::OpenSearch);
        }
        // `a`/`c` are QUEUE + WORKTREES chips.
        for f in [PaneId::Queue, PaneId::Worktrees] {
            assert_eq!(list_mode_action(&k(KeyCode::Char('a')), f), AppAction::OpenActionMenu);
            assert_eq!(list_mode_action(&k(KeyCode::Char('c')), f), AppAction::Create);
        }
        // TASKS shows only `[r]un [z]`, so `a`/`c` are inert there.
        assert_eq!(list_mode_action(&k(KeyCode::Char('a')), PaneId::Tasks), AppAction::None);
        assert_eq!(list_mode_action(&k(KeyCode::Char('c')), PaneId::Tasks), AppAction::None);
    }

    #[test]
    fn g_edges() {
        for f in LISTS {
            assert_eq!(list_mode_action(&k(KeyCode::Char('g')), f), AppAction::ScrollEdge(-1));
            assert_eq!(list_mode_action(&sk(KeyCode::Char('G')), f), AppAction::ScrollEdge(1));
        }
    }

    #[test]
    fn home_end_edges() {
        // Home/End are detail-only: they emit DetailScrollEdge (head/tail) on
        // every focused list pane and never map to ScrollEdge, so they can't
        // move the left-side list cursor.
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
    fn x_cancels_only_on_queue_and_ctrl_x_still_cycles() {
        // Plain `x` cancels on QUEUE (its `[x] cancel` chip); inert on TASKS /
        // WORKTREES (no cancel chip there).
        assert_eq!(list_mode_action(&k(KeyCode::Char('x')), PaneId::Queue), AppAction::CancelSelected);
        for f in [PaneId::Tasks, PaneId::Worktrees] {
            assert_eq!(list_mode_action(&k(KeyCode::Char('x')), f), AppAction::None);
        }
        // ctrl+x remains the forward sub-tab cycle (matched before plain `x`).
        assert_eq!(list_mode_action(&ck(KeyCode::Char('x')), F), AppAction::CycleSubTab(1));
    }

    #[test]
    fn unbound_keys_are_none() {
        // w/f/m moved to the action menu (parity with the Ink keymap tests). On
        // the QUEUE pane `t` is inert (a WORKTREES chip, keymap-gated). `r`/`x`
        // ARE bound on QUEUE now (re-queue / cancel), so they're not in this set.
        for c in ['w', 'f', 'm', 't'] {
            assert_eq!(list_mode_action(&k(KeyCode::Char(c)), PaneId::Queue), AppAction::None);
        }
    }

    #[test]
    fn r_runs_def_on_tasks_but_requeues_on_queue() {
        // `r` is a Run chip on both TASKS and QUEUE, meaning different verbs:
        // TASKS runs the highlighted def; QUEUE re-queues the selected task(s);
        // WORKTREES has no `[r]un` chip → inert.
        assert_eq!(list_mode_action(&k(KeyCode::Char('r')), PaneId::Tasks), AppAction::RunSelectedDef);
        assert_eq!(list_mode_action(&k(KeyCode::Char('r')), PaneId::Queue), AppAction::RequeueSelected);
        assert_eq!(list_mode_action(&k(KeyCode::Char('r')), PaneId::Worktrees), AppAction::None);
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
