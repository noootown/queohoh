use ratatui::style::{Color, Modifier, Style};

// Status + marker glyphs. All glyph literals live here (global constraint: no
// inline glyphs in components). Running list rows use an animated throbber
// instead of a static glyph; GLYPH_RUNNING is the static fallback used by the
// detail pane's lane-task rows.
pub const GLYPH_QUEUED: char = '○';
pub const GLYPH_NEEDS_INPUT: char = '?';
pub const GLYPH_DONE: char = '✓';
pub const GLYPH_FAILED: char = '✗';
pub const GLYPH_RUNNING: char = '▶';
pub const GLYPH_MAIN_SESSION: char = '⛓';
pub const GLYPH_MAIN_WT: char = '◆';
pub const GLYPH_DISCOVERY: char = '⏰';
/// Filled dot — colored by context (connection indicator, worktree state).
pub const GLYPH_DOT: char = '●';

/// Global-scope marker trailing a def-pick row (project-local defs render blank).
pub const MARKER_GLOBAL: &str = "(g)";

/// Central color palette (Catppuccin Mocha-inspired dark theme). The one place
/// colors are defined; components take `&Palette` and never name raw colors.
/// `info` doubles as the inline `` `code` `` color and `accent` as the URL color
/// in `markup.rs`. Fields are only ever added, never renamed.
#[derive(Debug, Clone)]
pub struct Palette {
    pub accent: Color,
    pub border: Color,
    pub border_focused: Color,
    pub dim: Color,
    pub error: Color,
    pub ok: Color,
    pub warn: Color,
    pub info: Color,
    pub fg: Color,
    pub selection_fg: Color,
    pub selection_bg: Color,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            accent: Color::Rgb(137, 180, 250),       // blue
            border: Color::Rgb(69, 71, 90),          // surface1
            border_focused: Color::Rgb(137, 180, 250),
            dim: Color::Rgb(127, 132, 156),          // overlay1
            error: Color::Rgb(243, 139, 168),        // red
            ok: Color::Rgb(166, 227, 161),           // green
            warn: Color::Rgb(249, 226, 175),         // yellow
            info: Color::Rgb(148, 226, 213),         // teal
            fg: Color::Rgb(205, 214, 244),           // text
            selection_fg: Color::Rgb(30, 30, 46),    // base
            selection_bg: Color::Rgb(137, 180, 250), // blue
        }
    }
}

impl Palette {
    /// Inverse-style highlight for the selected/active row.
    pub fn selection(&self) -> Style {
        Style::default().fg(self.selection_fg).bg(self.selection_bg)
    }

    /// Dimmed style for archived rows, hints, disabled items.
    pub fn dim_style(&self) -> Style {
        Style::default().fg(self.dim).add_modifier(Modifier::DIM)
    }

    /// Pane border color by focus state.
    pub fn border_style(&self, focused: bool) -> Style {
        Style::default().fg(if focused {
            self.border_focused
        } else {
            self.border
        })
    }
}
