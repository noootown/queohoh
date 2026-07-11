use ratatui::style::{Color, Modifier, Style};

// Status + marker glyphs. All glyph literals live here (global constraint: no
// inline glyphs in components). Running list rows use an animated throbber
// instead of a static glyph; GLYPH_RUNNING is the static fallback used by the
// detail pane's lane-task rows.
pub const GLYPH_QUEUED: char = '○';
/// Needs-input — `‼` (double exclamation) reads more urgent than the old `?`.
/// Single-width in the test terminal (verified via snapshot multi-width
/// annotations); if a future terminal renders it wide, fall back to a bold `?`.
pub const GLYPH_NEEDS_INPUT: char = '‼';
/// Done — a filled GREEN dot (user request). Shares the `●` glyph with the
/// worktree idle dot, but a different pane context and color path (`glyph_style`
/// here vs the worktree state styling), so they never conflict.
pub const GLYPH_DONE: char = '●';
pub const GLYPH_FAILED: char = '✗';
/// User-cancelled — `⊘` (circled slash, "stopped/void"), single-width, in warn
/// yellow. Distinct glyph from skipped so `glyph_style` (which keys on the char)
/// can color the two differently.
pub const GLYPH_CANCELLED: char = '⊘';
/// Chain-skipped — `⊝` (circled dash), single-width, dim (a passive non-run,
/// unlike the deliberate `⊘` cancel).
pub const GLYPH_SKIPPED: char = '⊝';
pub const GLYPH_RUNNING: char = '▶';
/// Lane has/resumes a main session — used in both the queue rows and the
/// worktree rows, so the two surfaces read as one marker. `⌂` (house): "the
/// main session lives here" (replaced ⛓, which read poorly; single-width).
pub const GLYPH_MAIN_SESSION: char = '⌂';
pub const GLYPH_DISCOVERY: char = '⏰';
/// Worktree has uncommitted changes (git status --porcelain non-empty).
pub const GLYPH_DIRTY: char = '±';
/// Filled dot — colored by context (connection indicator, worktree state).
pub const GLYPH_DOT: char = '●';
/// Magnifier prefixing the inline search-hint/input row. Double-width, but it is
/// the row's first column so it can't break column alignment.
pub const GLYPH_SEARCH: char = '🔍';
/// Block cursor at the end of the live search query in the hint row.
pub const GLYPH_CURSOR: char = '█';

/// Global-scope marker trailing a def-pick row (project-local defs render blank).
pub const MARKER_GLOBAL: &str = "(g)";

/// Horizontal-rule glyph. Matches the pane-border char so transcript code-fence
/// rules and the pane borders read as one system.
pub const RULE_CHAR: char = '─';
/// Leading rule run before a fenced-block language label
/// (`──────── bash ───────`).
pub const FENCE_RULE_PREFIX: usize = 8;
/// Minimum trailing rule run so a labeled rule never collapses to nothing on a
/// narrow pane.
pub const FENCE_RULE_MIN_TRAIL: usize = 3;

// Chip label words (the lowercase verb after the `(key)`). No inline literals in
// the component; the collapse chip picks LABEL_COLLAPSE / LABEL_EXPAND by state.
// A chip renders `[{key}] {label}` when there is room, degrading to the compact
// `[{key}]` form (labels dropped) on narrow panes. Icons were dropped — the
// emoji glyphs (➕ ⚙️ 🔽) rendered inconsistently across terminals and carried
// no meaning the label doesn't.
pub const BTN_LABEL_CREATE: &str = "create";
pub const BTN_LABEL_TASKS: &str = "tasks";
pub const BTN_LABEL_ACTIONS: &str = "actions";
pub const BTN_LABEL_RUN: &str = "run";
pub const BTN_LABEL_CANCEL: &str = "cancel";
pub const BTN_LABEL_COLLAPSE: &str = "collapse";
pub const BTN_LABEL_EXPAND: &str = "expand";

/// Idle placeholder label in the inline search-hint row (superfile-style),
/// rendered after the accent-bold `[/]` hotkey when the pane has no active
/// filter and is not being typed into.
pub const SEARCH_HINT_IDLE: &str = "filter";

// Pane title bases (emoji prefix included — titles are the one row where a
// double-width emoji can't break column alignment). ⚡ carries NO space before
// TASKS: the glyph is width-counted 2 but many terminals draw it narrow, so the
// pad cell alone reads as the gap (a literal space doubled it — user request).
pub const TITLE_QUEUE: &str = "📋 QUEUE";
pub const TITLE_TASKS: &str = "⚡TASKS";
pub const TITLE_WORKTREES: &str = "🌲 WORKTREES";
pub const TITLE_DETAIL: &str = "📄 DETAIL";

/// Semantic color table — ONE color per concept, applied uniformly across the
/// QUEUE / TASKS / WORKTREES panes (components take `&Palette`; never raw colors
/// in `panes.rs`):
///
/// | Color            | Concept                | Surfaces                                                                                   |
/// |------------------|------------------------|--------------------------------------------------------------------------------------------|
/// | `mauve`          | task / definition NAME | QUEUE def column; TASKS name column; WORKTREES `next: <name>` and last-task name WHEN a def |
/// | `accent` (blue)  | worktree IDENTITY      | QUEUE worktree column; WORKTREES name column                                                |
/// | `info` (teal)    | TIMESTAMPS only        | QUEUE timestamp + age; TASKS `⏰` schedule; WORKTREES commit-age, last-task age             |
/// | `meta`           | non-time metadata      | title-bar summaries; TASKS model column; WORKTREES `next:` lead; `⌂` marker; search query; settings values |
/// | `warn` (yellow)  | live / now             | `⏱` timers; throbber; `±` dirty marker; QUEUE `#N in lane` live text; markdown `{{jinja}}`  |
/// | `fg`             | prose / summaries      | QUEUE summary; WORKTREES last-task / `next` name WHEN a prompt (no definition)              |
/// | via `glyph_style`| status glyphs          | QUEUE/last-task status glyph (`● ✗ ▶ ○ ‼ ⊘ ⊝`)                                              |
///
/// `info` is deliberately reserved for timestamp-related text (user request);
/// every other informational column reads in `meta`.
///
/// The semantic palette components read from. The one place colors are defined;
/// components take `&Palette` and never name raw colors. `ok` doubles as the
/// inline `` `code` `` color, `accent` as the URL color, and `heading` as the
/// markdown heading color in `markup.rs`. Fields are only ever added, never
/// renamed. Concrete color SETS live in the theme profiles below ([`MOCHA`]);
/// [`THEME`] picks the active one.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub accent: Color,
    pub border: Color,
    pub border_focused: Color,
    pub dim: Color,
    pub error: Color,
    pub ok: Color,
    pub warn: Color,
    pub info: Color,
    pub meta: Color,
    pub fg: Color,
    pub mauve: Color,
    pub heading: Color,
    pub selection_fg: Color,
    pub selection_bg: Color,
}

/// Catppuccin Mocha-inspired dark profile (the original color set). The three
/// status slots (`ok`/`warn`/`error`) use raw terminal ANSI colors
/// (green/yellow/red) for a vivid, high-contrast look; the rest stay Catppuccin
/// RGB.
pub const MOCHA: Palette = Palette {
    accent: Color::Rgb(137, 180, 250),       // blue
    border: Color::Rgb(69, 71, 90),          // surface1
    border_focused: Color::Rgb(137, 180, 250),
    dim: Color::Rgb(147, 153, 178),          // overlay2 — brightest overlay; DIM
                                             // modifier deliberately not used
                                             // (user: grey-on-grey unreadable)
    error: Color::Red,                       // ANSI red — vivid status
    ok: Color::Green,                        // ANSI green — vivid status
    warn: Color::Yellow,                     // ANSI yellow — vivid status
    info: Color::Rgb(148, 226, 213),         // teal — timestamps ONLY
    meta: Color::Rgb(180, 190, 254),         // lavender — non-time metadata
    fg: Color::Rgb(205, 214, 244),           // text
    mauve: Color::Rgb(203, 166, 247),        // mauve
    heading: Color::Magenta,                 // ANSI magenta — markdown headings
    selection_fg: Color::Rgb(30, 30, 46),    // base
    selection_bg: Color::Rgb(137, 180, 250), // blue
};

/// The active theme profile. Re-theming the whole TUI = pointing this at a
/// different profile const (or adding a new one above) — nothing else names
/// colors.
pub const THEME: Palette = MOCHA;

impl Default for Palette {
    fn default() -> Self {
        THEME
    }
}

impl Palette {
    /// Inverse-style highlight for the selected/active row.
    pub fn selection(&self) -> Style {
        Style::default().fg(self.selection_fg).bg(self.selection_bg)
    }

    /// De-emphasis style for archived rows, empty states, disabled items. Uses a
    /// mid-brightness grey WITHOUT the terminal DIM modifier — dim-on-dark was
    /// unreadable. Informational columns (timestamps, args) get real palette
    /// colors instead of this.
    pub fn dim_style(&self) -> Style {
        Style::default().fg(self.dim)
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

/// Status-glyph color: done→ok, failed→error, running/needs-input→warn,
/// everything else→dim. The single place a glyph maps to a color.
pub fn glyph_style(glyph: char, p: &Palette) -> Style {
    match glyph {
        GLYPH_DONE => Style::default().fg(p.ok),
        GLYPH_FAILED => Style::default().fg(p.error),
        GLYPH_RUNNING => Style::default().fg(p.warn),
        // Needs-input is bold so the `‼` reads as urgent (also the graceful
        // degradation if a terminal renders it plainer than intended).
        GLYPH_NEEDS_INPUT => Style::default().fg(p.warn).add_modifier(Modifier::BOLD),
        GLYPH_CANCELLED => Style::default().fg(p.warn),
        GLYPH_SKIPPED => Style::default().fg(p.dim),
        GLYPH_QUEUED => Style::default().fg(p.dim),
        _ => Style::default().fg(p.dim),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph_style_colors_each_status_glyph() {
        let p = Palette::default();
        // Done green, failed vivid red, running/needs-input/cancelled warn,
        // skipped/queued dim. Needs-input is additionally bold (urgent `‼`).
        assert_eq!(glyph_style(GLYPH_DONE, &p), Style::default().fg(p.ok));
        assert_eq!(glyph_style(GLYPH_FAILED, &p), Style::default().fg(p.error));
        assert_eq!(glyph_style(GLYPH_RUNNING, &p), Style::default().fg(p.warn));
        assert_eq!(glyph_style(GLYPH_CANCELLED, &p), Style::default().fg(p.warn));
        assert_eq!(glyph_style(GLYPH_SKIPPED, &p), Style::default().fg(p.dim));
        assert_eq!(glyph_style(GLYPH_QUEUED, &p), Style::default().fg(p.dim));
        assert_eq!(
            glyph_style(GLYPH_NEEDS_INPUT, &p),
            Style::default().fg(p.warn).add_modifier(Modifier::BOLD),
        );
        // Cancelled and skipped use DISTINCT glyphs (glyph_style keys on the char,
        // so they must differ to color differently).
        assert_ne!(GLYPH_CANCELLED, GLYPH_SKIPPED);
    }

    #[test]
    fn new_status_glyphs_are_single_width() {
        use unicode_width::UnicodeWidthChar;
        for g in [GLYPH_NEEDS_INPUT, GLYPH_CANCELLED, GLYPH_SKIPPED, GLYPH_DONE] {
            assert_eq!(UnicodeWidthChar::width(g), Some(1), "glyph {g:?} must be single-width");
        }
    }
}
