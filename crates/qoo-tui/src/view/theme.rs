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
/// Verify-failed — `⊗` (circled times), single-width, in error red. Distinct
/// glyph from the worker `✗` so the two failure modes read apart, but the same
/// red because both are failures needing attention.
pub const GLYPH_VERIFY_FAILED: char = '⊗';
/// Session-limit hit — `$` (dollar sign), single-width, in error red. A
/// `failed` run whose result text reported Claude's own session/usage limit
/// (`worker.ts`'s `SESSION_LIMIT_RE`). Shares the `$` limit glyph with
/// out-of-budget by design: both mean "hit a spend/usage limit" — one visual
/// category; the detail pane's status text disambiguates which. Distinct from
/// the generic worker `✗` because retrying right away won't help.
pub const GLYPH_SESSION_LIMIT: char = '$';
/// Timed-out — `⧗` (hourglass), single-width, in error red. A `failed` run
/// that hit its configured `timeout` before finishing — distinct from the
/// generic worker `✗` so a wedged/slow task reads apart from an outright
/// error, but the same red because it's still a failure needing attention.
pub const GLYPH_TIMED_OUT: char = '⧗';
/// Out-of-budget — `$` (dollar sign), single-width, in error red. A `failed`
/// run whose result text reported Anthropic's credit-balance/out-of-credits
/// billing error (`worker.ts`'s `OUT_OF_BUDGET_RE`). Shares the `$` limit glyph
/// with session-limit by design (both are "hit a spend/usage limit"); the
/// detail pane's status text disambiguates a top-up-needed budget failure from
/// a wait-for-reset session limit. Same red because it's still a failure; the
/// money glyph lets a "rerun the limit-hit ones" sweep pick them out at a glance.
pub const GLYPH_OUT_OF_BUDGET: char = '$';
/// Provider-unavailable — `⊟` (squared minus), single-width, in error red. A
/// `failed` run whose configured provider/model (a non-claude adapter) was
/// unavailable — disabled in settings, missing credentials, or otherwise unable
/// to run (`selectors::PROVIDER_UNAVAILABLE_REASON`). Distinct from
/// `GLYPH_SESSION_LIMIT`/`GLYPH_OUT_OF_BUDGET` because those are Claude-account
/// states that clear on their own; this needs the provider itself fixed before a
/// rerun can succeed. Same red because it's still a failure needing attention.
pub const GLYPH_PROVIDER_UNAVAILABLE: char = '⊟';
pub const GLYPH_RUNNING: char = '▶';
/// Worktree has uncommitted changes (git status --porcelain non-empty).
pub const GLYPH_DIRTY: char = '±';
/// Worktree is protected from deletion (the project's main checkout or a name in
/// the project's `protected_worktrees`). Single-width shield in its own front
/// marker column beside the `±` dirty slot, mirroring `GLYPH_DIRTY` — so a
/// protected worktree can show both markers at once.
pub const GLYPH_PROTECTED: char = '⛨';
/// Discovery-backed task definition — front marker slot, mirroring `GLYPH_DIRTY`.
pub const GLYPH_DISCOVER: char = '⌕';
/// Worktree's committed work has been merged into the project's default branch
/// (vars.yaml `default_branch`) — front marker column beside `±`/`⛨`, in ok
/// green: "safe to clean up". `↣` (rightwards arrow with tail, single-width):
/// the branch flowed into the default branch. User-picked over `✓` (too
/// status-like) and `⎇`/`⋔` (read as "branch exists", not "merged").
pub const GLYPH_MERGED: char = '↣';
/// Worktree's PR is APPROVED (gh `reviewDecision === "APPROVED"`) but not yet
/// merged — shares `GLYPH_MERGED`'s front marker slot, also in ok green, but
/// yields to it (a merged PR shows `↣` even when it was also approved; see
/// `wt_merge_marker`). `✓` (check mark, single-width): the review passed. Here a
/// check reads as exactly the intended "approved" status, unlike on the merged
/// marker where it was rejected as too status-like for "flowed into the branch".
pub const GLYPH_APPROVED: char = '✓';
/// Filled dot — colored by context (connection indicator, worktree state).
pub const GLYPH_DOT: char = '●';
/// Magnifier prefixing the inline search-hint/input row. Double-width, but it is
/// the row's first column so it can't break column alignment.
pub const GLYPH_SEARCH: char = '🔍';
/// Block cursor at the end of the live search query in the hint row.
pub const GLYPH_CURSOR: char = '█';

/// Launcher entry markers — distinguish the two synthetic rows (New session /
/// Create Worktree) from resumable-session rows. Single-width glyphs from the
/// same family as the status glyphs (`⊘ ⊝ ⊗`) so column alignment holds across
/// terminals (unlike the double-width emoji dropped from the pane chips).
pub const GLYPH_NEW_SESSION: char = '✦';
pub const GLYPH_CREATE_WORKTREE: char = '⊕';

/// Dropdown affordance — a down chevron on the right of a closed select field.
pub const GLYPH_CHEVRON_DOWN: char = '▾';

/// Picker affordance — a right chevron on the right of a field whose activation
/// opens a separate modal (not an inline dropdown), e.g. the adhoc-create form's
/// session-continuation field, which opens the session picker.
pub const GLYPH_CHEVRON_RIGHT: char = '▸';

/// Head-of-lane next-queued task in the WORKTREES activity column — replaces the
/// old `next: ` text lead. Single-width rightwards arrow (U+2192): "what comes
/// next". Distinct from `▶` (running status), `↣` (merged-back), and `▸`
/// (picker affordance) so the three rightward marks stay readable apart.
pub const GLYPH_NEXT: char = '→';

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
// A chip renders `[{key}]{label}` when there is room, degrading to the compact
// `[{key}]` form (labels dropped) on narrow panes. Icons were dropped — the
// emoji glyphs (➕ ⚙️ 🔽) rendered inconsistently across terminals and carried
// no meaning the label doesn't.
pub const BTN_LABEL_CREATE: &str = "create";
pub const BTN_LABEL_TASKS: &str = "tasks";
pub const BTN_LABEL_RUN: &str = "run";
pub const BTN_LABEL_DISCOVER: &str = "discover";
pub const BTN_LABEL_RERUN: &str = "rerun";
pub const BTN_LABEL_GOTO: &str = "goto";
pub const BTN_LABEL_STOP: &str = "stop";
/// QUEUE archive/unarchive toggle. The label swaps on the FIRST (topmost)
/// selected row's state — `unarchive` when that row is already archived (the
/// verb `a` will restore it / the range), `archive` otherwise — so the chip
/// always reads as the action the key will take. The direction is threaded from
/// the selection into the chip renderer (see `view::panes::render_list_pane`).
pub const BTN_LABEL_ARCHIVE: &str = "archive";
pub const BTN_LABEL_UNARCHIVE: &str = "unarchive";
/// TASKS cron toggle. Rendered `[o]cron` — the key `o` is NOT the label's first
/// letter, so `button_chip` takes the `[z]collapse`-style branch (whole label
/// after the bracket) rather than stripping a leading key char.
pub const BTN_LABEL_CRON: &str = "cron";
pub const BTN_LABEL_REMOVE: &str = "remove";
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
/// | `mauve`          | task / definition NAME | QUEUE def column; TASKS name column; WORKTREES activity `→ <name>` and last-task name WHEN a def |
/// | `worktree`       | worktree IDENTITY NAME | QUEUE worktree column; WORKTREES name column                                                |
/// | `accent`         | generic UI accent      | selection bar; focused borders; active tab; dialog/menu borders; filter `>`; footer keys    |
/// | `info` (teal)    | TIMESTAMPS only        | QUEUE timestamp + age; TASKS Cron schedule text; WORKTREES commit-age, last-task age        |
/// | `meta`           | non-time metadata      | title-bar summaries; TASKS model column; WORKTREES `→` next lead; search query; settings values |
/// | `warn` (yellow)  | live / now             | `⏱` timers; throbber; `±` dirty marker; QUEUE `#N in lane` live text; markdown `{{jinja}}`  |
/// | `fg`             | prose / summaries      | QUEUE summary; WORKTREES last-task / `next` name WHEN a prompt (no definition)              |
/// | via `glyph_style`| status glyphs          | QUEUE/last-task status glyph (`● ✗ ▶ ○ ‼ ⊘ ⊝ ⊗ ⧗ $ ⊟`)                                    |
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
    /// Worktree-identity NAME columns only (QUEUE `worktree`, WORKTREES `name`).
    /// Split out from `accent` so it can be themed independently of the generic
    /// UI accent (selection bar, focused borders, tabs, prompts).
    pub worktree: Color,
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
    /// Dimmer companion bg for MARKED rows that are not the cursor (and not
    /// inside an anchored range) — see [`Palette::selection_muted`].
    pub selection_muted_bg: Color,
}

/// Catppuccin Mocha-inspired dark profile (the original color set). The three
/// status slots (`ok`/`warn`/`error`) use raw terminal ANSI colors
/// (green/yellow/red) for a vivid, high-contrast look; the rest stay Catppuccin
/// RGB.
pub const MOCHA: Palette = Palette {
    accent: Color::Rgb(137, 180, 250),       // blue
    worktree: Color::Rgb(137, 180, 250),     // worktree names = accent (no split)
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
    selection_muted_bg: Color::Rgb(54, 64, 102),
};

/// Brightened Mocha (user request: the original read too dim overall). Same
/// hues, lightness raised a step — every non-status slot moves up the
/// Catppuccin ladder (text→near-white, overlay2→subtext0, surface1→overlay0,
/// pastels lightened ~10%). The three status slots (`ok`/`warn`/`error`) stay
/// the raw ANSI colors — deliberately untouched. `dim` must stay clearly
/// dimmer than `fg` (it carries archived/empty de-emphasis), so it rises only
/// to subtext0 while `fg` goes near-white.
pub const MOCHA_BRIGHT: Palette = Palette {
    accent: Color::Rgb(166, 204, 255),       // blue, lightened
    worktree: Color::Rgb(166, 204, 255),     // worktree names = accent (no split)
    border: Color::Rgb(108, 112, 134),       // overlay0 — brighter frame
    border_focused: Color::Rgb(166, 204, 255),
    dim: Color::Rgb(166, 173, 200),          // subtext0 — brighter, still dim vs fg
    error: Color::Red,                       // ANSI red — vivid status (unchanged)
    ok: Color::Green,                        // ANSI green — vivid status (unchanged)
    warn: Color::Yellow,                     // ANSI yellow — vivid status (unchanged)
    info: Color::Rgb(178, 240, 229),         // teal, lightened — timestamps ONLY
    meta: Color::Rgb(205, 212, 255),         // lavender, lightened
    fg: Color::Rgb(230, 237, 255),           // near-white text
    mauve: Color::Rgb(221, 192, 255),        // mauve, lightened
    heading: Color::LightMagenta,            // brighter ANSI magenta headings
    selection_fg: Color::Rgb(30, 30, 46),    // base (dark text on the bright bar)
    selection_bg: Color::Rgb(166, 204, 255), // blue, lightened with accent
    selection_muted_bg: Color::Rgb(62, 74, 112),
};

/// Prism — a high-contrast rainbow profile (user pick), warm-leaning: light-orange
/// worktree NAME columns, spring-green task/def names, gold metadata, and pink
/// markdown headings are the warm slots; teal timestamps and the blue generic-UI
/// `accent` (selection bar, focused borders, tabs, prompts) are the cool anchors.
/// `fg` (near-white) is reserved for actions/tabs/chrome — prose and summaries
/// render in the terminal's default grey. The three status slots
/// (`ok`/`warn`/`error`) stay raw ANSI green/yellow/red (user: "keep the status
/// colors, those are already great"); names use a lighter spring green so they
/// never read as the "done" status dot.
pub const PRISM: Palette = Palette {
    accent: Color::Rgb(77, 166, 255),        // electric blue — generic UI accent
    worktree: Color::Rgb(255, 182, 133),     // lighter warm orange (user request) — worktree NAME columns only
    border: Color::Rgb(58, 63, 90),
    border_focused: Color::Rgb(77, 166, 255),
    dim: Color::Rgb(123, 131, 166),          // still clearly dimmer than fg
    error: Color::Red,                        // ANSI — status (kept)
    ok: Color::Green,                         // ANSI — status (kept)
    warn: Color::Yellow,                      // ANSI — status (kept)
    info: Color::Rgb(47, 230, 200),           // teal — timestamps ONLY (cool anchor)
    meta: Color::Rgb(230, 195, 74),           // gold — non-time metadata
    fg: Color::Rgb(238, 241, 255),            // near-white — reserved for actions/tabs/chrome
    // `mauve` is the legacy field name; PRISM colors task/def names spring GREEN
    // (a warm slot, distinct from the pure ANSI "done" green).
    mauve: Color::Rgb(123, 216, 143),         // spring green — task / def names
    heading: Color::Rgb(244, 114, 182),       // pink — markdown headings
    selection_fg: Color::Rgb(10, 10, 16),     // near-black text on the bright bar
    selection_bg: Color::Rgb(77, 166, 255),   // accent blue bar
    selection_muted_bg: Color::Rgb(38, 66, 112),
};

/// Neon Ice — the coldest, highest-contrast rainbow profile (user pick):
/// electric cyan worktree identity, indigo task/def names, sky-blue timestamps,
/// light-cyan metadata, and hot-pink headings over a near-black terminal. Same
/// status rule as [`PRISM`] — `ok`/`warn`/`error` stay raw ANSI green/yellow/red.
pub const NEON_ICE: Palette = Palette {
    accent: Color::Rgb(34, 211, 238),        // electric cyan
    worktree: Color::Rgb(34, 211, 238),      // worktree names = accent (no split)
    border: Color::Rgb(43, 53, 80),
    border_focused: Color::Rgb(34, 211, 238),
    dim: Color::Rgb(111, 123, 160),          // still clearly dimmer than fg
    error: Color::Red,                        // ANSI — status (kept)
    ok: Color::Green,                         // ANSI — status (kept)
    warn: Color::Yellow,                      // ANSI — status (kept)
    info: Color::Rgb(56, 189, 248),           // sky — timestamps ONLY
    meta: Color::Rgb(103, 232, 249),          // light cyan — non-time metadata
    fg: Color::Rgb(242, 247, 255),            // near-white text
    mauve: Color::Rgb(129, 140, 248),         // indigo — task / def names
    heading: Color::Rgb(244, 114, 182),       // hot pink — markdown headings
    selection_fg: Color::Rgb(5, 8, 15),       // near-black text on the bright bar
    selection_bg: Color::Rgb(34, 211, 238),   // accent cyan bar
    selection_muted_bg: Color::Rgb(26, 72, 88),
};

/// Synthwave — magenta + cyan accents on a deep-purple base (user pick): magenta
/// worktree identity, purple task/def names, teal timestamps, lavender metadata,
/// and cyan headings. Moodier/warmer than [`NEON_ICE`]; same status rule —
/// `ok`/`warn`/`error` stay raw ANSI green/yellow/red.
pub const SYNTHWAVE: Palette = Palette {
    accent: Color::Rgb(255, 95, 210),        // magenta
    worktree: Color::Rgb(255, 95, 210),      // worktree names = accent (no split)
    border: Color::Rgb(74, 58, 106),
    border_focused: Color::Rgb(255, 95, 210),
    dim: Color::Rgb(139, 123, 166),          // still clearly dimmer than fg
    error: Color::Red,                        // ANSI — status (kept)
    ok: Color::Green,                         // ANSI — status (kept)
    warn: Color::Yellow,                      // ANSI — status (kept)
    info: Color::Rgb(45, 212, 191),           // teal — timestamps ONLY
    meta: Color::Rgb(196, 181, 253),          // lavender — non-time metadata
    fg: Color::Rgb(253, 240, 255),            // near-white text (warm)
    mauve: Color::Rgb(167, 139, 250),         // purple — task / def names
    heading: Color::Rgb(34, 211, 238),        // cyan — markdown headings
    selection_fg: Color::Rgb(20, 10, 31),     // deep-purple-black text on the bar
    selection_bg: Color::Rgb(255, 95, 210),   // accent magenta bar
    selection_muted_bg: Color::Rgb(92, 42, 90),
};

/// The active theme profile. Re-theming the whole TUI = pointing this at a
/// different profile const (or adding a new one above) — nothing else names
/// colors.
pub const THEME: Palette = PRISM;

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

    /// Dimmer companion to [`selection`] for MARKED rows that are not the cursor
    /// (and not inside an anchored range) — the non-contiguous half of a bulk
    /// selection. Two-tone so the bright cursor bar stays locatable while marked
    /// rows read as selected-but-not-here. Uses `fg` (not `selection_fg`) because
    /// the muted bg is dark — near-white text keeps it readable.
    pub fn selection_muted(&self) -> Style {
        Style::default().fg(self.fg).bg(self.selection_muted_bg)
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

/// Status-glyph color: done→ok, failed/verify-failed→error, running/needs-input→
/// warn, everything else→dim. The single place a glyph maps to a color.
pub fn glyph_style(glyph: char, p: &Palette) -> Style {
    match glyph {
        GLYPH_DONE => Style::default().fg(p.ok),
        GLYPH_FAILED => Style::default().fg(p.error),
        // A failed done-condition is a failure too — same red, distinct glyph.
        GLYPH_VERIFY_FAILED => Style::default().fg(p.error),
        // Timeout and the shared `$` limit glyph (session-limit + out-of-budget)
        // are still failures — same red. `GLYPH_SESSION_LIMIT` isn't matched
        // separately: it equals `GLYPH_OUT_OF_BUDGET` (both `$`), so that arm
        // covers it (a separate arm would be an unreachable duplicate pattern).
        GLYPH_TIMED_OUT => Style::default().fg(p.error),
        GLYPH_OUT_OF_BUDGET => Style::default().fg(p.error),
        GLYPH_PROVIDER_UNAVAILABLE => Style::default().fg(p.error),
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
        assert_eq!(glyph_style(GLYPH_VERIFY_FAILED, &p), Style::default().fg(p.error));
        assert_eq!(glyph_style(GLYPH_SESSION_LIMIT, &p), Style::default().fg(p.error));
        assert_eq!(glyph_style(GLYPH_TIMED_OUT, &p), Style::default().fg(p.error));
        assert_eq!(glyph_style(GLYPH_OUT_OF_BUDGET, &p), Style::default().fg(p.error));
        assert_eq!(glyph_style(GLYPH_PROVIDER_UNAVAILABLE, &p), Style::default().fg(p.error));
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
        // Verify-failed, timed-out, provider-unavailable, and the shared `$`
        // limit glyph all share the error color with failed but MUST otherwise
        // read apart in the queue. Session-limit and out-of-budget DELIBERATELY
        // share `$` (one "hit a spend/usage limit" category, disambiguated by the
        // detail status text), so those two are asserted EQUAL.
        assert_ne!(GLYPH_VERIFY_FAILED, GLYPH_FAILED);
        assert_ne!(GLYPH_SESSION_LIMIT, GLYPH_FAILED);
        assert_ne!(GLYPH_TIMED_OUT, GLYPH_FAILED);
        assert_ne!(GLYPH_OUT_OF_BUDGET, GLYPH_FAILED);
        assert_ne!(GLYPH_PROVIDER_UNAVAILABLE, GLYPH_FAILED);
        assert_ne!(GLYPH_SESSION_LIMIT, GLYPH_VERIFY_FAILED);
        assert_ne!(GLYPH_TIMED_OUT, GLYPH_VERIFY_FAILED);
        assert_ne!(GLYPH_OUT_OF_BUDGET, GLYPH_VERIFY_FAILED);
        assert_ne!(GLYPH_PROVIDER_UNAVAILABLE, GLYPH_VERIFY_FAILED);
        assert_ne!(GLYPH_SESSION_LIMIT, GLYPH_TIMED_OUT);
        assert_eq!(GLYPH_OUT_OF_BUDGET, GLYPH_SESSION_LIMIT, "session-limit shares the $ limit glyph");
        assert_ne!(GLYPH_OUT_OF_BUDGET, GLYPH_TIMED_OUT);
        assert_ne!(GLYPH_PROVIDER_UNAVAILABLE, GLYPH_SESSION_LIMIT);
        assert_ne!(GLYPH_PROVIDER_UNAVAILABLE, GLYPH_TIMED_OUT);
        assert_ne!(GLYPH_PROVIDER_UNAVAILABLE, GLYPH_OUT_OF_BUDGET);
    }

    #[test]
    fn active_theme_keeps_ansi_status_colors() {
        // Invariant (user requirement): whatever the active profile, the three
        // status slots stay the raw ANSI green/yellow/red — a theme swap must not
        // silently recolor task status.
        let p = Palette::default();
        assert_eq!(p.ok, Color::Green);
        assert_eq!(p.warn, Color::Yellow);
        assert_eq!(p.error, Color::Red);
    }

    #[test]
    fn muted_selection_differs_from_bright() {
        let p = Palette::default();
        assert_ne!(p.selection(), p.selection_muted());
    }

    #[test]
    fn new_status_glyphs_are_single_width() {
        use unicode_width::UnicodeWidthChar;
        for g in [
            GLYPH_NEEDS_INPUT,
            GLYPH_CANCELLED,
            GLYPH_SKIPPED,
            GLYPH_VERIFY_FAILED,
            GLYPH_DONE,
            GLYPH_SESSION_LIMIT,
            GLYPH_TIMED_OUT,
            GLYPH_OUT_OF_BUDGET,
            GLYPH_PROVIDER_UNAVAILABLE,
        ] {
            assert_eq!(UnicodeWidthChar::width(g), Some(1), "glyph {g:?} must be single-width");
        }
    }
}
