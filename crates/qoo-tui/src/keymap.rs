use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::PaneId;

/// Pure key → action for `Mode::List`. Focus is invariantly one of the three
/// list panes (detail is display-only and never focused), so per-pane branching
/// is gone; `_focus` is retained for signature stability. Detail sub-tab cycling
/// is global via `ctrl+x`/`ctrl+z` → `CycleSubTab`. Project-tab cycling
/// (`CycleTab`) is driven by the stateful `ctrl+s` prefix in `App`, not here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppAction {
    MoveCursor(i32),
    ExtendSelection(i32),
    CyclePane(i32),
    SwitchTab(usize),
    CycleTab(i32),
    CycleSubTab(i32),
    OpenActionMenu,
    /// Open the task menu (`t`): the upgraded def picker over the active repo,
    /// carrying the selected worktree row's context when the worktrees pane holds
    /// focus. Routes to `App::open_task_menu`.
    OpenTaskMenu,
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
pub fn list_mode_action(key: &KeyEvent, _focus: PaneId) -> AppAction {
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
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
        KeyCode::Char('a') => AppAction::OpenActionMenu,
        KeyCode::Char('t') => AppAction::OpenTaskMenu,
        KeyCode::Char('c') => AppAction::Create,
        // `z` (plain) toggles collapse; `x` is reserved for a future delete
        // action and stays unbound.
        KeyCode::Char('z') => AppAction::ToggleCollapse,
        // g/G jump the list cursor to the first/last row; Home/End scroll ONLY
        // the detail pane (head/tail) and never touch the list selection.
        KeyCode::Char('g') => AppAction::ScrollEdge(-1),
        KeyCode::Char('G') => AppAction::ScrollEdge(1),
        KeyCode::Home => AppAction::DetailScrollEdge(-1),
        KeyCode::End => AppAction::DetailScrollEdge(1),
        KeyCode::Esc => AppAction::ClearEsc,
        KeyCode::Char('/') => AppAction::OpenSearch,
        KeyCode::Enter => AppAction::OpenActionMenu,
        KeyCode::Char('J') => AppAction::ExtendSelection(1),
        KeyCode::Char('K') => AppAction::ExtendSelection(-1),
        KeyCode::Down | KeyCode::Char('j') => {
            if shift { AppAction::ExtendSelection(1) } else { AppAction::MoveCursor(1) }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if shift { AppAction::ExtendSelection(-1) } else { AppAction::MoveCursor(-1) }
        }
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
    fn jk_arrows_move_cursor() {
        for f in LISTS {
            assert_eq!(list_mode_action(&k(KeyCode::Char('j')), f), AppAction::MoveCursor(1));
            assert_eq!(list_mode_action(&k(KeyCode::Down), f), AppAction::MoveCursor(1));
            assert_eq!(list_mode_action(&k(KeyCode::Char('k')), f), AppAction::MoveCursor(-1));
            assert_eq!(list_mode_action(&k(KeyCode::Up), f), AppAction::MoveCursor(-1));
        }
    }

    #[test]
    fn extend_selection_bindings() {
        for f in LISTS {
            assert_eq!(list_mode_action(&sk(KeyCode::Down), f), AppAction::ExtendSelection(1));
            assert_eq!(list_mode_action(&sk(KeyCode::Up), f), AppAction::ExtendSelection(-1));
            assert_eq!(list_mode_action(&sk(KeyCode::Char('J')), f), AppAction::ExtendSelection(1));
            assert_eq!(list_mode_action(&sk(KeyCode::Char('K')), f), AppAction::ExtendSelection(-1));
        }
    }

    #[test]
    fn hl_arrows_are_unbound() {
        // Detail is display-only; horizontal focus nav is gone.
        for f in LISTS {
            assert_eq!(list_mode_action(&k(KeyCode::Char('l')), f), AppAction::None);
            assert_eq!(list_mode_action(&k(KeyCode::Right), f), AppAction::None);
            assert_eq!(list_mode_action(&k(KeyCode::Char('h')), f), AppAction::None);
            assert_eq!(list_mode_action(&k(KeyCode::Left), f), AppAction::None);
        }
    }

    #[test]
    fn enter_opens_action_menu() {
        for f in LISTS {
            assert_eq!(list_mode_action(&k(KeyCode::Enter), f), AppAction::OpenActionMenu);
        }
    }

    #[test]
    fn a_c_slash_esc_help() {
        for f in LISTS {
            assert_eq!(list_mode_action(&k(KeyCode::Char('a')), f), AppAction::OpenActionMenu);
            assert_eq!(list_mode_action(&k(KeyCode::Char('c')), f), AppAction::Create);
            assert_eq!(list_mode_action(&k(KeyCode::Char('?')), f), AppAction::Help);
            assert_eq!(list_mode_action(&k(KeyCode::Esc), f), AppAction::ClearEsc);
            assert_eq!(list_mode_action(&k(KeyCode::Char('/')), f), AppAction::OpenSearch);
        }
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
    fn x_is_reserved_and_unbound_but_ctrl_x_still_cycles() {
        // Plain `x` is reserved for a future delete action → unbound.
        assert_eq!(list_mode_action(&k(KeyCode::Char('x')), F), AppAction::None);
        // ctrl+x remains the forward sub-tab cycle.
        assert_eq!(list_mode_action(&ck(KeyCode::Char('x')), F), AppAction::CycleSubTab(1));
    }

    #[test]
    fn unbound_keys_are_none() {
        // r/w/f/m moved to the action menu (parity with the Ink keymap tests).
        // `t` opens the task menu and `s` opens the settings overlay, so both are
        // bound; plain `x` is reserved/unbound (ctrl+x is the sub-tab cycle).
        for c in ['r', 'w', 'f', 'm', 'x'] {
            assert_eq!(list_mode_action(&k(KeyCode::Char(c)), PaneId::Queue), AppAction::None);
        }
    }

    #[test]
    fn t_opens_task_menu() {
        for f in LISTS {
            assert_eq!(list_mode_action(&k(KeyCode::Char('t')), f), AppAction::OpenTaskMenu);
        }
    }
}
