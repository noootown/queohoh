//! Core UI state and mode types for the TUI `App`.
//!
//! Pure data types: pane identifiers, drag/selection state, per-tab UI state,
//! and the `Mode` enum describing which overlay/input the app is in. Moved out
//! of `app/mod.rs` verbatim as part of the module split (no behavior change).

use ratatui::layout::Rect;

use crate::event::Cmd;
use crate::ipc::types::DefinitionSummary;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneId {
    Queue,
    Tasks,
    Worktrees,
    Detail,
}

/// What a left-mouse drag is currently manipulating, recorded on `Down` over a
/// draggable target and cleared on `Up`. Generalizes the old scrollbar-only drag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragKind {
    /// Proportional scrollbar drag on a pane (behavior unchanged).
    Scrollbar(PaneId),
    /// Horizontal pane divider: `0` = queue/tasks, `1` = tasks/worktrees.
    DividerH(usize),
    /// Vertical divider between the left pane stack and DETAIL.
    DividerV,
    /// Text selection in the DETAIL pane content area: the drag extends
    /// `App::detail_selection.cursor`; the matching `Up` copies to the clipboard.
    DetailSelect,
}

/// A point in the DETAIL pane's WRAPPED content, in ABSOLUTE display-line
/// coordinates (survives scrolling — the same text stays selected as the window
/// moves under it). `cell` is a 0-based terminal cell column relative to the
/// line's first cell; it is mapped to a char index only when text is extracted,
/// so multi-width chars are handled once at that boundary rather than smeared
/// through the selection logic. `Copy` so `DetailSelection` stays trivial.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetailPoint {
    pub line: usize,
    pub cell: usize,
}

/// An in-progress or finalized text selection in the DETAIL pane. `anchor` is
/// where the drag began, `cursor` where it currently is; the pair is ordered at
/// read time (`ordered`). It persists after the drag-release (stays highlighted)
/// until a plain click, or a content / sub-tab / focus change, clears it — so a
/// scroll keeps the highlight anchored to the same wrapped lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetailSelection {
    pub anchor: DetailPoint,
    pub cursor: DetailPoint,
}

impl DetailSelection {
    /// `(start, end)` ordered by `(line, cell)` — reading order.
    pub fn ordered(&self) -> (DetailPoint, DetailPoint) {
        if (self.anchor.line, self.anchor.cell) <= (self.cursor.line, self.cursor.cell) {
            (self.anchor, self.cursor)
        } else {
            (self.cursor, self.anchor)
        }
    }

    /// A zero-width selection (press with no movement) — a plain click, which
    /// clears rather than copies.
    pub(super) fn is_click(&self) -> bool {
        self.anchor == self.cursor
    }
}

/// Render-feedback geometry the DETAIL view publishes each frame so mouse
/// routing can resolve a `(col, row)` into a [`DetailPoint`] against the SAME
/// wrapped lines that were just drawn. Interior-mutability twin of `hit` /
/// `detail_wrapped_len` (see [`App::detail_geom`]); always fresh because every
/// state change redraws before the next event is read.
#[derive(Debug, Clone, Default)]
pub struct DetailGeom {
    /// The content region (below the sub-tab chip row, inside the border).
    pub area: Rect,
    /// Absolute index of the first wrapped display line visible in `area`.
    pub window_start: usize,
    /// Every wrapped display line's text (the WHOLE content, not just the
    /// window) so absolute line indices resolve and clamp correctly.
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListPane {
    Queue = 0,
    Tasks = 1,
    Worktrees = 2,
}

impl ListPane {
    pub fn idx(self) -> usize {
        self as usize
    }
}

/// Which detail-pane context is showing. Discriminants index `TabUiState.sub_tab`
/// (one remembered sub-tab per kind). See `detail::derive_context`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailKind {
    Run = 0,
    Definition = 1,
    Worktree = 2,
    Empty = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Selection {
    pub cursor: usize,
    pub anchor: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TabUiState {
    /// Invariant: always one of the three list panes (`Queue`/`Tasks`/
    /// `Worktrees`). Detail is display-only and can never be focused — no code
    /// path sets `focus = PaneId::Detail`. `TabUiState` is session-only (never
    /// serialized), so there is no persisted value to coerce; the invariant is
    /// upheld at the mutation sites (`set_focus`, `CyclePane`).
    pub focus: PaneId,
    pub last_list_pane: ListPane,
    pub selections: [Selection; 3],
    pub search: [String; 3],
    pub sub_tab: [usize; 4], // indexed by DetailKind (enum lands in Task 9)
    pub scroll_offset: usize,
    /// Row cursor for the DETAIL pane's worktree lane-task list (`j`/`k` when the
    /// detail shows selectable rows). Indexes the ordered lane tasks; reset to 0
    /// whenever the WORKTREES selection changes, and clamped at read time so a
    /// shrunk task list can never index out of range. Only meaningful in the
    /// Worktree detail context (other contexts scroll with `j`/`k` instead).
    pub detail_row: usize,
}

impl Default for TabUiState {
    fn default() -> Self {
        Self {
            focus: PaneId::Queue,
            last_list_pane: ListPane::Queue,
            selections: [Selection::default(); 3],
            search: [String::new(), String::new(), String::new()],
            sub_tab: [0; 4],
            scroll_offset: 0,
            detail_row: 0,
        }
    }
}

/// Subset of the contract `Mode`. Variants are only ever
/// added. `PartialEq` is intentionally not derived: `AddTask` carries a
/// `MultilineInput` and `Form` a `FormState`, neither of which is `PartialEq`;
/// nothing compares `Mode` by value (tests use `matches!`).
#[derive(Debug, Clone, Default)]
pub enum Mode {
    #[default]
    List,
    /// Filter-typing for one list pane. Printable keys append to
    /// `TabUiState.search[pane]`; the pane title shows `/query█`.
    Search { pane: ListPane },
    /// Full-screen keymap overlay; any key returns to `List`.
    Help,
    /// Read-only model-alias settings overlay (`s`). Any key returns to `List`,
    /// exactly like `Help`. The data it shows lives in `App::settings`, fetched
    /// once on first open.
    Settings,
    /// Single-target (or bulk) action menu over the last-focused list pane's
    /// selection. Lazyvim-style picker: `query` filters `items` by label (empty
    /// = all), `index` is the highlighted row WITHIN the filtered view (reset to
    /// 0 on every query change), and `preview_scroll` is the right (description)
    /// panel's first visible wrapped line (reset to 0 whenever the query or the
    /// highlighted row changes). Disabled rows are inert on Enter/click.
    ActionMenu {
        title: String,
        items: Vec<crate::action_menu::ActionItem>,
        index: usize,
        query: String,
        preview_scroll: usize,
    },
    /// Unified destructive-confirmation dialog (remove worktree, bulk remove,
    /// queue cancel). `title` names the verb; `body` are the message lines (built
    /// per-verb at open time — the branch/warning lines, the truncated name list,
    /// the running-will-be-stopped summary); `confirm_label` is the Confirm
    /// button's verb; `action` is the frozen payload fired on confirm. `focus`
    /// is the highlighted button (defaults to Confirm on open): Left/Right/Tab
    /// move it; Enter activates the focused button; `y`/`n` are always-on
    /// accelerators; Esc dismisses (unadvertised). A click on either button acts
    /// regardless of focus; a click inside the body is inert; an outside click
    /// dismisses.
    Confirm {
        title: String,
        body: Vec<String>,
        confirm_label: String,
        action: ConfirmAction,
        focus: crate::hit::ButtonKind,
    },
    /// New adhoc-task prompt. Enter submits (enqueue), Shift+Enter inserts a
    /// newline into the multiline `editor`, Esc cancels.
    AddTask {
        worktree: Option<String>,
        /// Pin: resume this session (lineage-resolved at spawn). None = fresh.
        resume_session_id: Option<String>,
        /// Human label of the picked session, for the modal title.
        resume_label: Option<String>,
        editor: crate::view::multiline_input::MultilineInput,
    },
    /// Task menu / def picker over the active repo (opened by `t`). Lazyvim-style
    /// picker: `query` filters `defs` by name (empty = all), `index` is the
    /// highlighted row WITHIN the filtered view (reset to 0 on every query
    /// change), and `preview_scroll` is the right (prompt) panel's first visible
    /// wrapped line (reset on query/highlight changes). `defs` is the repo's
    /// summaries in server (alphabetical) order; `worktree`/`branch` are the
    /// explicit-target context (from the selected worktree row) that drives the
    /// chosen def's args as FIXED values.
    DefPick {
        defs: Vec<DefinitionSummary>,
        index: usize,
        worktree: Option<String>,
        branch: Option<String>,
        query: String,
        preview_scroll: usize,
    },
    /// Per-arg entry form for a chosen def (Task 18 constructs it; its key
    /// handling + render land in Task 19/20).
    DefArgs { form: crate::view::args_form::ArgsForm },
    /// Session picker (`r` on a worktree row): pick a resumable Claude session to
    /// carry into `Mode::AddTask`, or start fresh. Row 0 is ALWAYS the synthetic
    /// "New session" (fresh task); the loaded `items` follow it. `query` filters
    /// the loaded session labels only (row 0 stays visible regardless); `index`
    /// is the highlighted row over the VIEW (`0` = New session, `1..` = filtered
    /// items). `loading` gates the placeholder row until [`Event::SessionsLoaded`]
    /// (matched on `worktree`) fills `items`. `repo`/`worktree` are the frozen
    /// target the fetch and the chosen AddTask carry.
    SessionPick {
        repo: String,
        worktree: String,
        items: Vec<crate::event::SessionChoice>,
        loading: bool,
        index: usize,
        query: String,
        /// Focused button in the bottom row (defaults to `Confirm` = Next on
        /// open). Tab toggles it; Enter fires the focused button.
        focus: crate::hit::ButtonKind,
    },
    /// Reusable bordered typed form (Phase 4/5). `state` holds the fields, focus,
    /// caret, dropdown, and validation error (see [`crate::view::form::FormState`]);
    /// `action` is the frozen payload the Primary button fires once the form
    /// validates. Key/click handling lives in `app/form.rs`; rendering in
    /// `view/form.rs`.
    Form {
        state: crate::view::form::FormState,
        action: FormAction,
    },
}

/// What a validated [`Mode::Form`] fires on its Primary button. Each variant
/// carries the frozen launcher context (repo/worktree) captured when the form
/// opened; the field VALUES (model, prompt, branch name) come from `validate()`
/// at fire time. See `App::fire_form_action`.
#[derive(Debug, Clone)]
pub enum FormAction {
    /// New task (fresh or resumed) on an existing `worktree`. Fields:
    /// `[model dropdown, prompt textarea]`. `resume_session_id` pins a session.
    NewSession {
        repo: String,
        worktree: String,
        resume_session_id: Option<String>,
    },
    /// Create a new worktree in `repo`, then enqueue a first task into it.
    /// Fields: `[branch/name input, model dropdown, prompt textarea]`.
    CreateWorktree { repo: String },
}

/// The frozen payload a [`Mode::Confirm`] fires when confirmed. Each variant
/// reproduces exactly the `Cmd`s its former dedicated mode produced; the display
/// text lives in `Mode::Confirm.body`, so nothing here is render-only.
#[derive(Debug, Clone)]
pub enum ConfirmAction {
    /// Single `removeWorktree` (one `Cmd::Rpc`, no range to clear).
    RemoveWorktree { repo: String, worktree: String },
    /// One `removeWorktree` per name in an `RpcSeq` (verb "removed"); clears the
    /// WORKTREES range first. `names` are the frozen raw worktree names.
    BulkRemoveWorktrees { repo: String, names: Vec<String> },
    /// The frozen per-task skip/stop RPCs in one `RpcSeq` (verb "cancelled");
    /// clears the QUEUE range first.
    CancelTasks { calls: Vec<crate::event::RpcCall> },
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Update {
    pub dirty: bool,
    pub cmds: Vec<Cmd>,
}

/// Per-call overrides for `App::dispatch_rpc`. Contract addition (M2).
#[derive(Debug, Default, Clone)]
pub struct RpcOpts {
    pub timeout_ms: Option<u64>,
    pub timeout_is_ok: bool,
    pub invalidate_defs_for: Option<String>,
}
