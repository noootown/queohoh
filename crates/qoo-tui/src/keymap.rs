use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::PaneId;

/// Contract enum + two additions (see plan): `CycleSubTab(i32)` — `{`/`}` cycle
/// the detail sub-tab (digits are project tabs globally); `FocusBack` — detail
/// h/← returns to `last_list_pane`, resolved in `apply_action`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppAction {
    MoveCursor(i32),
    ExtendSelection(i32),
    FocusPane(PaneId),
    FocusBack,
    CyclePane(i32),
    SwitchTab(usize),
    CycleTab(i32),
    CycleSubTab(i32),
    OpenActionMenu,
    Create,
    OpenSearch,
    ClearEsc,
    Scroll(i32),
    ScrollEdge(i32),
    SwitchSubTab(usize),
    Help,
    Quit,
    None,
}

/// KeyEvent → AppAction in `Mode::List`. Pure; per-pane semantics resolved here
/// (lists vs detail), per-tab state resolved by `App::apply_action`.
/// Version note: crossterm delivers shifted letters as uppercase `Char('J')`
/// with `SHIFT` set; we match on the char and treat the modifier as advisory.
pub fn list_mode_action(key: &KeyEvent, focus: PaneId) -> AppAction {
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let detail = matches!(focus, PaneId::Detail);
    match key.code {
        KeyCode::Tab => AppAction::CyclePane(1),
        KeyCode::BackTab => AppAction::CyclePane(-1),
        KeyCode::Char(c @ '1'..='9') => AppAction::SwitchTab(c as usize - '1' as usize),
        KeyCode::Char('[') => AppAction::CycleTab(-1),
        KeyCode::Char(']') => AppAction::CycleTab(1),
        KeyCode::Char('{') => AppAction::CycleSubTab(-1),
        KeyCode::Char('}') => AppAction::CycleSubTab(1),
        KeyCode::Char('q') => AppAction::Quit,
        KeyCode::Char('?') => AppAction::Help,
        KeyCode::Char('a') => AppAction::OpenActionMenu,
        KeyCode::Char('c') => AppAction::Create,
        KeyCode::Char('g') => AppAction::ScrollEdge(-1),
        KeyCode::Char('G') => AppAction::ScrollEdge(1),
        KeyCode::Esc => AppAction::ClearEsc,
        KeyCode::Char('/') if !detail => AppAction::OpenSearch,
        KeyCode::Enter if !detail => AppAction::OpenActionMenu,
        KeyCode::Char('J') if !detail => AppAction::ExtendSelection(1),
        KeyCode::Char('K') if !detail => AppAction::ExtendSelection(-1),
        KeyCode::Down | KeyCode::Char('j') => {
            if detail {
                AppAction::Scroll(1)
            } else if shift {
                AppAction::ExtendSelection(1)
            } else {
                AppAction::MoveCursor(1)
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if detail {
                AppAction::Scroll(-1)
            } else if shift {
                AppAction::ExtendSelection(-1)
            } else {
                AppAction::MoveCursor(-1)
            }
        }
        KeyCode::Right | KeyCode::Char('l') => {
            if detail { AppAction::None } else { AppAction::FocusPane(PaneId::Detail) }
        }
        KeyCode::Left | KeyCode::Char('h') => {
            if detail { AppAction::FocusBack } else { AppAction::None }
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
    const LISTS: [PaneId; 3] = [PaneId::Queue, PaneId::Tasks, PaneId::Worktrees];
    const ALL: [PaneId; 4] = [PaneId::Queue, PaneId::Tasks, PaneId::Worktrees, PaneId::Detail];

    #[test]
    fn q_quits_from_every_pane() {
        for f in ALL {
            assert_eq!(list_mode_action(&k(KeyCode::Char('q')), f), AppAction::Quit);
        }
    }

    #[test]
    fn digits_switch_project_tabs_globally() {
        for f in ALL {
            for n in 1..=9u32 {
                let c = char::from_digit(n, 10).unwrap();
                assert_eq!(
                    list_mode_action(&k(KeyCode::Char(c)), f),
                    AppAction::SwitchTab((n - 1) as usize)
                );
            }
        }
    }

    #[test]
    fn brackets_cycle_project_tabs_and_braces_cycle_sub_tabs() {
        assert_eq!(list_mode_action(&k(KeyCode::Char('[')), PaneId::Queue), AppAction::CycleTab(-1));
        assert_eq!(list_mode_action(&k(KeyCode::Char(']')), PaneId::Queue), AppAction::CycleTab(1));
        assert_eq!(list_mode_action(&k(KeyCode::Char('{')), PaneId::Detail), AppAction::CycleSubTab(-1));
        assert_eq!(list_mode_action(&k(KeyCode::Char('}')), PaneId::Detail), AppAction::CycleSubTab(1));
    }

    #[test]
    fn tab_cycles_panes() {
        assert_eq!(list_mode_action(&k(KeyCode::Tab), PaneId::Queue), AppAction::CyclePane(1));
        assert_eq!(list_mode_action(&k(KeyCode::BackTab), PaneId::Detail), AppAction::CyclePane(-1));
    }

    #[test]
    fn jk_arrows_move_cursor_in_lists() {
        for f in LISTS {
            assert_eq!(list_mode_action(&k(KeyCode::Char('j')), f), AppAction::MoveCursor(1));
            assert_eq!(list_mode_action(&k(KeyCode::Down), f), AppAction::MoveCursor(1));
            assert_eq!(list_mode_action(&k(KeyCode::Char('k')), f), AppAction::MoveCursor(-1));
            assert_eq!(list_mode_action(&k(KeyCode::Up), f), AppAction::MoveCursor(-1));
        }
    }

    #[test]
    fn jk_arrows_scroll_in_detail() {
        assert_eq!(list_mode_action(&k(KeyCode::Char('j')), PaneId::Detail), AppAction::Scroll(1));
        assert_eq!(list_mode_action(&k(KeyCode::Down), PaneId::Detail), AppAction::Scroll(1));
        assert_eq!(list_mode_action(&k(KeyCode::Char('k')), PaneId::Detail), AppAction::Scroll(-1));
        assert_eq!(list_mode_action(&k(KeyCode::Up), PaneId::Detail), AppAction::Scroll(-1));
        // shift+arrow in detail keeps scrolling (no extend) — parity with TS.
        assert_eq!(list_mode_action(&sk(KeyCode::Down), PaneId::Detail), AppAction::Scroll(1));
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
    fn horizontal_focus_moves() {
        for f in LISTS {
            assert_eq!(list_mode_action(&k(KeyCode::Char('l')), f), AppAction::FocusPane(PaneId::Detail));
            assert_eq!(list_mode_action(&k(KeyCode::Right), f), AppAction::FocusPane(PaneId::Detail));
            // h/← on a list pane stays put.
            assert_eq!(list_mode_action(&k(KeyCode::Char('h')), f), AppAction::None);
            assert_eq!(list_mode_action(&k(KeyCode::Left), f), AppAction::None);
        }
        assert_eq!(list_mode_action(&k(KeyCode::Char('h')), PaneId::Detail), AppAction::FocusBack);
        assert_eq!(list_mode_action(&k(KeyCode::Left), PaneId::Detail), AppAction::FocusBack);
        assert_eq!(list_mode_action(&k(KeyCode::Char('l')), PaneId::Detail), AppAction::None);
    }

    #[test]
    fn enter_opens_action_menu_on_lists_only() {
        for f in LISTS {
            assert_eq!(list_mode_action(&k(KeyCode::Enter), f), AppAction::OpenActionMenu);
        }
        assert_eq!(list_mode_action(&k(KeyCode::Enter), PaneId::Detail), AppAction::None);
    }

    #[test]
    fn a_c_slash_esc_help() {
        for f in ALL {
            assert_eq!(list_mode_action(&k(KeyCode::Char('a')), f), AppAction::OpenActionMenu);
            assert_eq!(list_mode_action(&k(KeyCode::Char('c')), f), AppAction::Create);
            assert_eq!(list_mode_action(&k(KeyCode::Char('?')), f), AppAction::Help);
            assert_eq!(list_mode_action(&k(KeyCode::Esc), f), AppAction::ClearEsc);
        }
        for f in LISTS {
            assert_eq!(list_mode_action(&k(KeyCode::Char('/')), f), AppAction::OpenSearch);
        }
        assert_eq!(list_mode_action(&k(KeyCode::Char('/')), PaneId::Detail), AppAction::None);
    }

    #[test]
    fn g_edges_everywhere() {
        for f in ALL {
            assert_eq!(list_mode_action(&k(KeyCode::Char('g')), f), AppAction::ScrollEdge(-1));
            assert_eq!(list_mode_action(&sk(KeyCode::Char('G')), f), AppAction::ScrollEdge(1));
        }
    }

    #[test]
    fn unbound_keys_are_none() {
        // r/s/w/f/m/t moved to the action menu (parity with the Ink keymap tests).
        for c in ['r', 's', 'w', 'f', 'm', 't', 'z', '0'] {
            assert_eq!(list_mode_action(&k(KeyCode::Char(c)), PaneId::Queue), AppAction::None);
        }
    }
}
