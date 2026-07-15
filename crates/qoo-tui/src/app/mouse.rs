//! Mouse-event routing and pointer-driven interactions for `App`.
//!
//! Click/drag/wheel dispatch (`on_mouse`), pane-divider and scrollbar drags, and
//! the DETAIL-pane text-selection lifecycle plus its scroll helpers. Split out of
//! `app/mod.rs` verbatim (no behavior change).

use super::*;

impl App {
    /// Current detail kind + sub-tab for the active tab (needed for scroll inversion).
    /// The detail context for the current selection — the same derivation the
    /// renderer uses (last-focused list pane + its selection). Shared by the
    /// sub-tab math and the `j`/`k`/`Enter` detail-row handlers.
    pub(super) fn current_detail_context(&self) -> crate::detail::DetailContext {
        let c = crate::view::compute(self);
        match (&self.snapshot, &c.active_name) {
            (Some(snap), Some(name)) => crate::detail::derive_context(
                snap,
                name,
                c.ui.last_list_pane,
                &c.queue,
                &c.worktrees,
                &c.defs,
                &c.ui.selections,
            ),
            _ => crate::detail::DetailContext::Empty,
        }
    }

    pub(super) fn detail_kind_and_subtab(&self) -> (DetailKind, usize) {
        let c = crate::view::compute(self);
        let ctx = match (&self.snapshot, &c.active_name) {
            (Some(snap), Some(name)) => crate::detail::derive_context(
                snap,
                name,
                c.ui.last_list_pane,
                &c.queue,
                &c.worktrees,
                &c.defs,
                &c.ui.selections,
            ),
            _ => crate::detail::DetailContext::Empty,
        };
        let kind = ctx.kind();
        let sub = crate::detail::clamp_sub_tab(c.ui.sub_tab[kind as usize], kind);
        (kind, sub)
    }

    /// `Scroll(delta)` in the detail pane. Bottom-anchored views invert so k =
    /// older. Applies to the active tab's `scroll_offset`.
    pub(crate) fn detail_scroll(&mut self, delta: i32) -> bool {
        let (kind, sub) = self.detail_kind_and_subtab();
        let step = if crate::detail::bottom_anchored(kind, sub) { -delta } else { delta };
        // Clamp BOTH ends: 0 and the render-fed max. Without the upper clamp the
        // stored offset kept growing on over-scroll past the edge, and the user
        // had to scroll back through the phantom distance before the view moved.
        let max = self.detail_max_scroll.get();
        let ui = self.ui();
        let next = ((ui.scroll_offset as i64 + step as i64).max(0) as usize).min(max);
        if next == ui.scroll_offset {
            return false;
        }
        ui.scroll_offset = next;
        true
    }

    /// Scroll the detail pane to its edge. Driven by `Home`/`End`
    /// (`DetailScrollEdge`). dir < 0 = head/oldest, dir > 0 =
    /// tail/end. Jumps to the render-fed max (not an unclamped sentinel, which
    /// left the stored offset far past the edge — same phantom-scroll bug class
    /// as `detail_scroll`'s missing upper clamp).
    pub(crate) fn detail_scroll_edge(&mut self, dir: i32) -> bool {
        let (kind, sub) = self.detail_kind_and_subtab();
        let bottom = crate::detail::bottom_anchored(kind, sub);
        let to_head = dir < 0;
        let max = self.detail_max_scroll.get();
        // Bottom-anchored: head = max offset, tail = 0. Top-anchored: reverse.
        let offset = if bottom {
            if to_head { max } else { 0 }
        } else if to_head {
            0
        } else {
            max
        };
        let ui = self.ui();
        if ui.scroll_offset == offset {
            return false;
        }
        ui.scroll_offset = offset;
        true
    }

    /// Reset the detail scroll to its anchor. Called on selection / sub-tab /
    /// focus change so a new selection always starts at its default view.
    pub(crate) fn reset_scroll(&mut self) {
        self.ui().scroll_offset = 0;
        // Sub-tab / selection changes route through here — clear any text
        // selection so it never straddles freshly-swapped content.
        self.detail_selection = None;
    }

    /// Resolve a mouse `(col, row)` to a [`DetailPoint`] ONLY when it lands inside
    /// the rendered detail content area — used to START a selection on `Down` so a
    /// click on the chip-row gap (which still hits `PaneBody(Detail)`) or an empty
    /// pane never begins one. `None` when there is no content or the point is out
    /// of the content rect.
    fn detail_point_in_content(&self, col: u16, row: u16) -> Option<DetailPoint> {
        let g = self.detail_geom.borrow();
        if g.lines.is_empty() || g.area.width == 0 || g.area.height == 0 {
            return None;
        }
        if !g.area.contains(Position { x: col, y: row }) {
            return None;
        }
        let row_off = (row - g.area.y) as usize;
        let line = (g.window_start + row_off).min(g.lines.len() - 1);
        let cell = (col - g.area.x) as usize;
        Some(DetailPoint { line, cell })
    }

    /// Resolve a mouse `(col, row)` to a [`DetailPoint`], CLAMPED into the content
    /// area — used while DRAGGING so a drag past an edge still extends the
    /// selection to the nearest line/column (no auto-scroll). `None` only when
    /// there is no rendered content.
    fn detail_point_clamped(&self, col: u16, row: u16) -> Option<DetailPoint> {
        let g = self.detail_geom.borrow();
        if g.lines.is_empty() || g.area.width == 0 || g.area.height == 0 {
            return None;
        }
        let r = row.clamp(g.area.y, g.area.bottom().saturating_sub(1));
        let c = col.clamp(g.area.x, g.area.right().saturating_sub(1));
        let row_off = (r - g.area.y) as usize;
        let line = (g.window_start + row_off).min(g.lines.len() - 1);
        let cell = (c - g.area.x) as usize;
        Some(DetailPoint { line, cell })
    }

    /// The current detail selection rendered to text — each selected wrapped
    /// display line sliced by the selection's cell columns, joined with `\n`.
    /// Empty when there is no selection.
    fn detail_selection_text(&self) -> String {
        match &self.detail_selection {
            Some(sel) => {
                let g = self.detail_geom.borrow();
                crate::view::detail::extract_selection(&g.lines, sel)
            }
            None => String::new(),
        }
    }

    /// Begin a detail text selection at `(col, row)` on `Down`. Starts a
    /// zero-width selection (replacing any prior one, which clears its highlight)
    /// and arms the drag. When the click is outside the content area it clears an
    /// existing selection instead. Returns whether a redraw is needed.
    fn start_detail_selection(&mut self, col: u16, row: u16) -> bool {
        match self.detail_point_in_content(col, row) {
            Some(pt) => {
                let redraw = self.detail_selection.is_some(); // clearing the old highlight
                self.detail_selection = Some(DetailSelection { anchor: pt, cursor: pt });
                // A new selection invalidates any pending post-copy fade timer —
                // its `SelectionExpired` will arrive with a stale epoch and no-op.
                self.selection_epoch += 1;
                self.drag = Some(DragKind::DetailSelect);
                redraw
            }
            None => self.detail_selection.take().is_some(),
        }
    }

    /// Extend the active detail selection to `(col, row)` on `Drag`. Returns true
    /// only when the cursor moved to a new cell (avoids redraw churn within one
    /// cell).
    fn drag_detail_selection(&mut self, col: u16, row: u16) -> bool {
        let Some(pt) = self.detail_point_clamped(col, row) else { return false };
        match self.detail_selection.as_mut() {
            Some(sel) if sel.cursor != pt => {
                sel.cursor = pt;
                true
            }
            _ => false,
        }
    }

    /// Finalize a detail text-selection drag on `Up`. A plain click (anchor ==
    /// cursor, no movement) clears the selection and copies nothing; a real drag
    /// copies the text (OSC 52 + `pbcopy` via [`Cmd::CopyClipboard`]), reports
    /// the size on the status line, and keeps the highlight for a 1s fade (an
    /// epoch-guarded [`Cmd::ExpireSelection`] clears it unless a newer selection
    /// started in the meantime). Returns whether a redraw is needed.
    fn finish_detail_selection(&mut self, cmds: &mut Vec<Cmd>) -> bool {
        let Some(sel) = self.detail_selection else { return false };
        if sel.is_click() {
            // Plain click: the zero-width selection shows nothing, so clearing it
            // is visually a no-op. Copy nothing.
            self.detail_selection = None;
            return false;
        }
        let text = self.detail_selection_text();
        if text.is_empty() {
            return false;
        }
        let n_lines = text.matches('\n').count() + 1;
        self.status_line = Some(if n_lines > 1 {
            format!("copied {n_lines} lines")
        } else {
            format!("copied {} chars", text.chars().count())
        });
        cmds.push(Cmd::CopyClipboard { text });
        cmds.push(Cmd::ExpireSelection { epoch: self.selection_epoch, delay_ms: 1000 });
        true
    }

    /// Detail content height ≈ terminal rows − 6 (header + footer + borders +
    /// chip row), floored at 1. Drives the tail read size, not layout.
    pub(crate) fn detail_height(&self) -> usize {
        (self.size.1 as usize).saturating_sub(6).max(1)
    }

    /// Tail lines to read: 4 windows of scrollback behind the visible region.
    pub(crate) fn tail_lines(&self) -> usize {
        (self.detail_height() * 4).max(1)
    }

    /// The pane a hit target belongs to (for wheel/drag routing). Row/PaneBody/
    /// Scrollbar* all map to their owning pane; everything else is `None`.
    fn pane_of_target(t: &HitTarget) -> Option<PaneId> {
        match t {
            HitTarget::Row(pane, _) => Some(match pane {
                ListPane::Queue => PaneId::Queue,
                ListPane::Tasks => PaneId::Tasks,
                ListPane::Worktrees => PaneId::Worktrees,
            }),
            HitTarget::PaneBody(p)
            | HitTarget::ScrollbarThumb(p)
            | HitTarget::ScrollbarTrack(p) => Some(*p),
            _ => None,
        }
    }

    /// Route a mouse event through the previous frame's hit map. Clicks focus/
    /// select/switch, the wheel scrolls the pane under the cursor without
    /// stealing focus, and scrollbar drags map proportionally.
    pub(super) fn on_mouse(&mut self, m: crossterm::event::MouseEvent) -> Update {
        use crossterm::event::{
            KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind as K,
        };
        // Text-input modals own the mouse: only a left-click routes (Confirm ≡
        // Enter, Cancel ≡ Esc, outside ≡ cancel); every other mouse kind is inert
        // so a move/drag never disturbs the field or closes the popup. Handling
        // mouse here (before the typing arms) is what keeps clicks out of the
        // `tui_input` field entirely.
        if matches!(self.mode, Mode::AddTask { .. }) {
            if let K::Down(MouseButton::Left) = m.kind {
                match self.hit.hit(m.column, m.row).cloned() {
                    // The adhoc-task prompt registers a Confirm button (≡ Enter);
                    // Modal is inert and an outside click cancels.
                    Some(HitTarget::Button(crate::hit::ButtonKind::Confirm)) => {
                        return self
                            .update(Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
                    }
                    Some(HitTarget::Button(crate::hit::ButtonKind::Cancel)) => {
                        return self
                            .update(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
                    }
                    Some(HitTarget::Modal) => return Update { dirty: false, cmds: vec![] },
                    _ => {
                        // Click outside the popup cancels (same as esc).
                        self.mode = Mode::List;
                        return Update { dirty: true, cmds: vec![] };
                    }
                }
            }
            return Update { dirty: false, cmds: vec![] };
        }
        let mut cmds = Vec::new();
        let target = self.hit.hit(m.column, m.row).cloned();
        let dirty = match m.kind {
            // The def-pick popup owns every click while open.
            K::Down(MouseButton::Left) if matches!(self.mode, Mode::DefPick { .. }) => {
                return self.route_def_pick_click(target);
            }
            // The session picker owns every click while open.
            K::Down(MouseButton::Left) if matches!(self.mode, Mode::SessionPick { .. }) => {
                return self.route_session_pick_click(target);
            }
            // The args form owns every click while open: route to its hit
            // targets; a click hitting nothing (outside the popup) cancels.
            K::Down(MouseButton::Left) if matches!(self.mode, Mode::DefArgs { .. }) => {
                return match target {
                    Some(t) => self.def_args_click(&t),
                    None => {
                        self.mode = Mode::List;
                        Update { dirty: true, cmds: vec![] }
                    }
                };
            }
            // The bordered form owns every click while open: route to its hit
            // targets; a click hitting nothing (outside the popup) cancels.
            K::Down(MouseButton::Left) if matches!(self.mode, Mode::Form { .. }) => {
                return match target {
                    Some(t) => self.form_click(&t),
                    None => {
                        self.mode = Mode::List;
                        Update { dirty: true, cmds: vec![] }
                    }
                };
            }
            // The unified confirm dialog owns every click: the Confirm button
            // fires the frozen action, the Cancel button and any outside click
            // dismiss, a click inside the body is inert. Both buttons act
            // regardless of which one has keyboard focus.
            K::Down(MouseButton::Left) if matches!(self.mode, Mode::Confirm { .. }) => {
                return match target {
                    Some(HitTarget::Button(crate::hit::ButtonKind::Confirm)) => {
                        self.confirm_dialog_fire()
                    }
                    Some(HitTarget::Button(crate::hit::ButtonKind::Cancel)) => {
                        self.mode = Mode::List;
                        Update { dirty: true, cmds: vec![] }
                    }
                    Some(HitTarget::Modal) => Update { dirty: false, cmds: vec![] },
                    _ => {
                        self.mode = Mode::List;
                        Update { dirty: true, cmds: vec![] }
                    }
                };
            }
            // Help/settings overlays own every click too: a click inside the modal
            // is inert; anything else (including outside → None) dismisses.
            K::Down(MouseButton::Left)
                if matches!(self.mode, Mode::Help | Mode::Settings) =>
            {
                return match target {
                    Some(HitTarget::Modal) => Update { dirty: false, cmds: vec![] },
                    _ => {
                        self.mode = Mode::List;
                        Update { dirty: true, cmds: vec![] }
                    }
                };
            }
            K::Down(MouseButton::Left) => match target {
                Some(HitTarget::Modal) => false, // click inside an overlay: inert (menus/forms wire up in M2/M3)
                None => {
                    // Click hitting nothing while an overlay is open closes it
                    // (same staged semantics as esc).
                    if !matches!(self.mode, Mode::List) {
                        self.mode = Mode::List;
                        true
                    } else {
                        false
                    }
                }
                Some(HitTarget::Tab(i)) => {
                    return self.apply_action(crate::keymap::AppAction::SwitchTab(i));
                }
                Some(HitTarget::SubTab(i)) => self.set_sub_tab_clamped(i, &mut cmds),
                Some(HitTarget::DetailLaneTask(i)) => {
                    // Click a worktree-detail lane task: select ONLY (same as
                    // moving the j/k cursor there) — jumping to the queue detail
                    // is a separate, explicit action (Enter / `A::OpenDetailRow`).
                    self.ui().detail_row = i;
                    true
                }
                Some(HitTarget::PaneBody(p)) => {
                    // Detail is display-only: clicking its body must not steal
                    // focus (wheel scrolling over it still works — that routes by
                    // hover, not focus). A press in the content area instead begins
                    // a tmux-style text selection.
                    if p == PaneId::Detail {
                        self.start_detail_selection(m.column, m.row)
                    } else {
                        self.set_focus(p);
                        true
                    }
                }
                Some(HitTarget::Row(pane, i)) => {
                    let focus = match pane {
                        ListPane::Queue => PaneId::Queue,
                        ListPane::Tasks => PaneId::Tasks,
                        ListPane::Worktrees => PaneId::Worktrees,
                    };
                    let shift = m.modifiers.contains(crossterm::event::KeyModifiers::SHIFT);
                    self.set_focus(focus);
                    if shift {
                        // Extend: keep (or seed) the anchor, move the cursor to i.
                        // A shift-click is not part of a double-click sequence.
                        self.last_click = None;
                        let sel = self.ui().selections[pane as usize];
                        let anchor = Some(sel.anchor.unwrap_or(sel.cursor));
                        let len = self.visible_len(pane);
                        let cursor = if len == 0 { 0 } else { i.min(len - 1) };
                        self.ui().selections[pane as usize] = Selection {
                            cursor,
                            anchor: if anchor == Some(cursor) { None } else { anchor },
                        };
                        self.ui().scroll_offset = 0;
                        self.schedule_run_read(&mut cmds, 120);
                        true
                    } else {
                        // Single click selects only. A real double-click — a second
                        // click on the SAME ROW IDENTITY within DOUBLE_CLICK_MS —
                        // fires the row's default action (same target as
                        // `open_actions_or_run`). Keying on identity (resolved
                        // from the clicked index) not the index means a resort
                        // between clicks can't fire on a row that merely slid
                        // into the clicked slot.
                        let now = self.now_ms;
                        let identity = self.row_identity(pane, i);
                        let double = match (&self.last_click, &identity) {
                            (Some((lp, lid, lt)), Some(id)) => {
                                *lp == pane
                                    && lid == id
                                    && now.saturating_sub(*lt) < DOUBLE_CLICK_MS
                            }
                            _ => false,
                        };
                        self.set_cursor(pane, i, &mut cmds);
                        if double {
                            // Consume the sequence so a third click starts fresh.
                            // A tasks-pane row runs its def directly; a queue row
                            // resumes its Claude session; a worktrees row is a
                            // no-op here (its `r`/`g`/`x` hotkeys act on it instead).
                            self.last_click = None;
                            let u = self.open_actions_or_run();
                            cmds.extend(u.cmds);
                        } else {
                            // Arm on the clicked row's identity (None → nothing to
                            // match against next click; disarms the sequence).
                            self.last_click = identity.map(|id| (pane, id, now));
                        }
                        true
                    }
                }
                Some(HitTarget::ScrollbarThumb(p)) | Some(HitTarget::ScrollbarTrack(p)) => {
                    self.drag = Some(DragKind::Scrollbar(p));
                    self.drag_to_offset(p, m.row, &mut cmds)
                }
                Some(HitTarget::PaneDividerH(i)) => {
                    self.drag = Some(DragKind::DividerH(i));
                    self.drag_divider_h(i, m.row)
                }
                Some(HitTarget::PaneDividerV) => {
                    self.drag = Some(DragKind::DividerV);
                    self.drag_divider_v(m.column)
                }
                Some(HitTarget::PaneButton(p, btn)) => {
                    // A title-bar button behaves exactly like pressing its hotkey
                    // with that pane focused. `Create`/`Actions` need the focus
                    // (they read `last_list_pane`); `Collapse` is focus-independent
                    // (its outcome — collapsing pane P — matches pressing `x` with
                    // P focused regardless), so it skips the focus/scroll reset.
                    let lp = match p {
                        PaneId::Queue => ListPane::Queue,
                        PaneId::Tasks => ListPane::Tasks,
                        PaneId::Worktrees => ListPane::Worktrees,
                        PaneId::Detail => return Update { dirty: false, cmds }, // no detail buttons
                    };
                    match btn {
                        crate::hit::PaneButton::Collapse => {
                            // Not in any pane's bulk-doable set — a bulk
                            // selection on `lp` refuses (status line) rather
                            // than collapsing/expanding out from under it.
                            if !self.bulk_blocked(lp, crate::hit::PaneButton::Collapse) {
                                self.toggle_collapse(lp, &mut cmds);
                            }
                            true
                        }
                        crate::hit::PaneButton::Create => {
                            self.set_focus(p);
                            return self.apply_action(crate::keymap::AppAction::Create);
                        }
                        crate::hit::PaneButton::Tasks => {
                            self.set_focus(p);
                            return self.apply_action(crate::keymap::AppAction::OpenTaskMenu);
                        }
                        crate::hit::PaneButton::Run => {
                            self.set_focus(p);
                            // Run means re-queue on QUEUE, run-def on TASKS, new
                            // worktree-targeted task on WORKTREES (the keymap's
                            // per-pane `r` split).
                            let action = match lp {
                                ListPane::Queue => crate::keymap::AppAction::RequeueSelected,
                                ListPane::Worktrees => crate::keymap::AppAction::NewTaskOnWorktree,
                                ListPane::Tasks => crate::keymap::AppAction::RunSelectedDef,
                            };
                            return self.apply_action(action);
                        }
                        crate::hit::PaneButton::Discover => {
                            self.set_focus(p);
                            return self.apply_action(crate::keymap::AppAction::DiscoverSelectedDef);
                        }
                        crate::hit::PaneButton::Goto => {
                            self.set_focus(p);
                            // Goto means resume-the-session on QUEUE, open-in-
                            // tmux on WORKTREES (the keymap's per-pane `g` split;
                            // TASKS has no Goto chip, so this arm never fires there).
                            let action = match lp {
                                ListPane::Queue => crate::keymap::AppAction::GotoQueue,
                                ListPane::Worktrees => crate::keymap::AppAction::GotoWorktree,
                                ListPane::Tasks => crate::keymap::AppAction::GotoWorktree,
                            };
                            return self.apply_action(action);
                        }
                        crate::hit::PaneButton::Cancel => {
                            self.set_focus(p);
                            return self.apply_action(crate::keymap::AppAction::CancelSelected);
                        }
                        crate::hit::PaneButton::Archive => {
                            self.set_focus(p);
                            return self.apply_action(crate::keymap::AppAction::ArchiveSelected);
                        }
                        crate::hit::PaneButton::Remove => {
                            self.set_focus(p);
                            return self.apply_action(crate::keymap::AppAction::RemoveSelectedWorktree);
                        }
                    }
                }
                Some(_) => false, // MenuItem/FormField/DropdownItem/Button: M2/M3
            },
            K::Drag(MouseButton::Left) => match self.drag {
                Some(DragKind::Scrollbar(p)) => self.drag_to_offset(p, m.row, &mut cmds),
                Some(DragKind::DividerH(i)) => self.drag_divider_h(i, m.row),
                Some(DragKind::DividerV) => self.drag_divider_v(m.column),
                Some(DragKind::DetailSelect) => self.drag_detail_selection(m.column, m.row),
                None => false,
            },
            K::Up(MouseButton::Left) => {
                // Drag ends. A divider drag changed the per-project layout → write
                // it through to disk (once, on release — not on every drag frame).
                // Scrollbar drags change no layout, so they don't persist. A detail
                // text selection copies + reports on release.
                match self.drag.take() {
                    Some(DragKind::DividerH(_)) | Some(DragKind::DividerV) => {
                        cmds.push(self.save_layout_cmd());
                        false
                    }
                    Some(DragKind::DetailSelect) => self.finish_detail_selection(&mut cmds),
                    _ => false,
                }
            }
            // An open picker owns the wheel: over the preview panel it scrolls
            // the preview, over the left panel it moves the selection; it never
            // reaches the panes beneath the modal.
            K::ScrollDown | K::ScrollUp
                if matches!(self.mode, Mode::DefPick { .. } | Mode::DefArgs { .. }) =>
            {
                let delta: i32 = if matches!(m.kind, K::ScrollDown) { 1 } else { -1 };
                return self.menu_wheel(target, delta);
            }
            // The session picker owns the wheel: over its body it moves the
            // selection one row (clamped, non-circular); it never reaches the
            // panes beneath the modal.
            K::ScrollDown | K::ScrollUp if matches!(self.mode, Mode::SessionPick { .. }) => {
                let delta: i32 = if matches!(m.kind, K::ScrollDown) { 1 } else { -1 };
                return self.session_pick_wheel(target, delta);
            }
            K::ScrollDown | K::ScrollUp => {
                let delta: i32 = if matches!(m.kind, K::ScrollDown) { 1 } else { -1 };
                match target.as_ref().and_then(Self::pane_of_target) {
                    // Detail scrolls `WHEEL_STEP` lines per tick — 1 line per tick
                    // read as sluggish over long transcripts. The picker/run-form
                    // preview panels share the same step (see `menu_wheel`).
                    Some(PaneId::Detail) => self.detail_scroll(delta * WHEEL_STEP),
                    Some(p) => {
                        // Wheel scrolls the pane UNDER the cursor without focus change.
                        let pane = match p {
                            PaneId::Queue => ListPane::Queue,
                            PaneId::Tasks => ListPane::Tasks,
                            PaneId::Worktrees => ListPane::Worktrees,
                            PaneId::Detail => unreachable!(),
                        };
                        let cur = self.ui().selections[pane as usize].cursor as i64;
                        let next = (cur + delta as i64).max(0) as usize;
                        self.set_cursor(pane, next, &mut cmds)
                    }
                    None => false,
                }
            }
            _ => false,
        };
        Update { dirty, cmds }
    }

    /// Proportional drag: offset = (y − track_top) × scrollable ÷ track_h,
    /// clamped. Lists map to the cursor; detail maps to the window start and
    /// converts per anchor.
    fn drag_to_offset(&mut self, pane: PaneId, y: u16, cmds: &mut Vec<Cmd>) -> bool {
        let track = self
            .hit
            .iter()
            .find(|(_, t)| matches!(t, HitTarget::ScrollbarTrack(p) if *p == pane))
            .map(|(r, _)| *r);
        let Some(track) = track else { return false };
        let track_h = track.height.max(1) as usize;
        let rel = (y.max(track.y) - track.y) as usize;
        match pane {
            PaneId::Detail => {
                let total = crate::view::detail::detail_content_len(self);
                let height = self.detail_height();
                let scrollable = total.saturating_sub(height);
                let start = (rel * scrollable / track_h).min(scrollable);
                let (kind, sub) = self.detail_kind_and_subtab();
                let offset = if crate::detail::bottom_anchored(kind, sub) {
                    scrollable - start
                } else {
                    start
                };
                if self.ui().scroll_offset == offset {
                    return false;
                }
                self.ui().scroll_offset = offset;
                true
            }
            PaneId::Queue | PaneId::Tasks | PaneId::Worktrees => {
                let list = match pane {
                    PaneId::Queue => ListPane::Queue,
                    PaneId::Tasks => ListPane::Tasks,
                    _ => ListPane::Worktrees,
                };
                let len = self.visible_len(list);
                let scrollable = len.saturating_sub(1);
                let cursor = (rel * scrollable / track_h).min(scrollable);
                self.set_cursor(list, cursor, cmds)
            }
        }
    }

    /// The left-pane body rectangle geometry the dividers move within. The header
    /// is a single row, the footer a single row (`view::render`), so the body
    /// starts at row 1 and the left column starts at column 0.
    fn body_height(&self) -> u16 {
        self.size.1.saturating_sub(2)
    }

    /// Drag a horizontal pane divider to absolute mouse row `y`. `which` selects
    /// the boundary (0 = queue/tasks, 1 = tasks/worktrees). The requested height is
    /// the rows between the body top and the drop point; `pane_layout` re-clamps it
    /// so every pane keeps its minimum. Overrides are canonicalized to the realized
    /// (clamped) heights so a drag past the limit can't accumulate stale slack.
    pub(super) fn drag_divider_h(&mut self, which: usize, y: u16) -> bool {
        // A boundary adjacent to a collapsed pane can't move — the collapsed pane
        // is pinned to COLLAPSED_H. Ignore the drag rather than fight the clamp.
        // `which` 0 = queue/tasks (panes 0,1); 1 = tasks/worktrees (panes 1,2).
        if self.collapsed[which] || self.collapsed[which + 1] {
            return false;
        }
        const BODY_TOP: u16 = 1; // header occupies row 0
        let body_h = self.body_height();
        let rel = y.saturating_sub(BODY_TOP);
        let before = crate::selectors::pane_layout(
            body_h,
            self.queue_h_override,
            self.tasks_h_override,
            self.collapsed,
        );
        let (mut q_ov, mut t_ov) = (self.queue_h_override, self.tasks_h_override);
        match which {
            // queue/tasks boundary → queue height = rows above the boundary.
            0 => q_ov = Some(rel),
            // tasks/worktrees boundary → tasks height = rows between the two
            // boundaries (drop point minus the current queue height).
            _ => t_ov = Some(rel.saturating_sub(before.queue_h)),
        }
        let after = crate::selectors::pane_layout(body_h, q_ov, t_ov, self.collapsed);
        self.queue_h_override = Some(after.queue_h);
        self.tasks_h_override = Some(after.tasks_h);
        after != before
    }

    /// Drag the vertical divider to absolute mouse column `x`: the drop column
    /// becomes the first column of DETAIL, i.e. the left-column width. Clamped so
    /// neither side collapses.
    fn drag_divider_v(&mut self, x: u16) -> bool {
        let clamped = crate::selectors::clamp_left_cols(self.size.0, x);
        if self.left_cols == Some(clamped) {
            return false;
        }
        self.left_cols = Some(clamped);
        true
    }
}
