use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::view::theme::{FENCE_RULE_MIN_TRAIL, FENCE_RULE_PREFIX, Palette, RULE_CHAR};

/// Normalize one line of arbitrary captured text (test-runner output inside a
/// report/transcript) for cell rendering: resolve `\r` overwrites the way a
/// terminal would (keep only the final carriage-return segment), strip ANSI
/// CSI/OSC/two-byte escape sequences, expand tabs, and drop any remaining
/// control chars. Without this, ratatui silently skips the zero-width ESC byte
/// but PRINTS the printable tail of the sequence (`[2m`…) and the wrap math
/// counts phantom columns — raw ANSI text renders as interleaved garbage.
pub fn sanitize_display_line(line: &str) -> String {
    let line = line.strip_suffix('\r').unwrap_or(line); // CRLF file read as \n-split
    let line = line.rsplit('\r').next().unwrap_or(line); // spinner overwrites: last wins
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.peek() {
                Some('[') => {
                    // CSI: consume params/intermediates through the final byte @..~.
                    chars.next();
                    for n in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&n) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    // OSC: consume through BEL or the ST (`ESC \`) terminator.
                    chars.next();
                    while let Some(n) = chars.next() {
                        if n == '\x07' {
                            break;
                        }
                        if n == '\x1b' {
                            chars.next();
                            break;
                        }
                    }
                }
                Some(_) => {
                    chars.next(); // two-byte escape (ESC x)
                }
                None => {}
            },
            '\t' => out.push_str("    "),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Per-line fence context, precomputed over the whole transcript by
/// [`fence_states`] so a window into the middle of a code block styles
/// correctly (the renderer only ever sees a slice, [`crate::view::detail`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineCtx {
    /// Ordinary line outside any fence — styled by [`style_line`].
    Text,
    /// A ```` ``` ```` delimiter. `lang` carries the info string of an *opening*
    /// fence (empty info → `None`); a *closing* or bare-opening fence carries
    /// `None`. Rendered as a horizontal rule — labeled when `lang` is `Some`.
    Fence { lang: Option<String> },
    /// A content line inside a fence, tagged with the block's language (empty
    /// string for an unlabeled block).
    Fenced { lang: String },
    /// A section header on the run detail's `info` sub-tab (e.g. `Run`,
    /// `Timing`), styled bold in the heading color and never key/value-split.
    /// Distinct from a markdown `#` heading — no marks are shown.
    Header,
    /// A `key   value` row on the definition detail's config sub-tab. `key_col`
    /// is the char index where the value column begins (the key is left-padded to
    /// this width across all rows), so [`style_config_line`] can color the key
    /// column distinctly from the value without re-parsing a separator.
    Config { key_col: usize },
    /// A queue-style task row in the worktree detail's lane-task list. The line
    /// text is the task's display NAME (def name or prompt summary); the ctx
    /// carries the pieces the styler needs to render a
    /// `glyph name … Created Age Live` row like the queue pane: `glyph` (status,
    /// colored by `glyph_style`), `is_def` (mauve name vs fg summary), the fixed
    /// right-aligned `created` (absolute local time) / `age` (relative) / `live`
    /// (elapsed for running, `#N in lane` for queued, else empty) columns, and
    /// whether this is the detail row cursor (`selected` → the whole row inverts
    /// with the palette selection style). See [`style_lane_task_line`].
    LaneTask { glyph: char, is_def: bool, created: String, age: String, live: String, selected: bool },
    /// The column-header row above the worktree detail's lane-task list —
    /// `Task Created Age Live` in the pane's de-emphasis dim, aligned cell-for-cell
    /// with [`LineCtx::LaneTask`] (no header over the leading glyph slot). Chrome,
    /// never a cursor row. See [`style_lane_header_line`].
    LaneHeader,
}

/// One cheap pass over the full transcript classifying each line. A line whose
/// trimmed content starts with ```` ``` ```` toggles the fence; the info string
/// after the backticks on an *opening* fence names the language. A second
/// ```` ``` ```` closes the block (so a nested fence just ends the first — there
/// is no nesting). An unclosed fence at EOF leaves trailing lines as `Fenced`.
pub fn fence_states(lines: &[String]) -> Vec<LineCtx> {
    fence_states_from(lines, false)
}

/// [`fence_states`] with an explicit starting state, for a window read from the
/// middle of a file (the transcript tail): when `starts_in_fence` the opener
/// scrolled out of the window, so the first line is treated as fenced content
/// with an unknown (empty) language, and the first bare ```` ``` ```` CLOSES that
/// fence. All other callers read from line 0 and pass `false` via [`fence_states`].
///
/// After the pass, unlabeled / `text` / `plain` fence bodies that *look like
/// markdown* (agent thinking often opens a bare ```` ``` ```` and then dumps
/// prose) are re-tagged `lang = "markdown"` so paint uses the prose styler
/// instead of plain/code accents.
pub fn fence_states_from(lines: &[String], starts_in_fence: bool) -> Vec<LineCtx> {
    let mut out = Vec::with_capacity(lines.len());
    // Some(lang) while inside a fence; None outside. An unknown-language open
    // (window began mid-fence) carries the empty lang a bare ``` opener produces.
    let mut open: Option<String> = starts_in_fence.then(String::new);
    // Index of first body line of the current fence (for reclass on close/EOF).
    // Mid-window: opener scrolled out → body starts at line 0.
    let mut body_start: Option<usize> = starts_in_fence.then_some(0);
    for line in lines {
        if let Some(rest) = line.trim_start().strip_prefix("```") {
            if open.is_none() {
                let info = rest.trim();
                // First token of the info string is the language (ignore attrs).
                let lang_tok = info.split_whitespace().next().unwrap_or("");
                let lang = if lang_tok.is_empty() {
                    None
                } else {
                    Some(lang_tok.to_string())
                };
                open = Some(lang.clone().unwrap_or_default());
                body_start = Some(out.len() + 1); // next pushed line is first body
                out.push(LineCtx::Fence { lang });
            } else {
                // Closing fence — maybe upgrade unlabeled body to markdown.
                if let Some(start) = body_start {
                    reclass_fence_body_if_markdown(&mut out, start, lines);
                }
                open = None;
                body_start = None;
                out.push(LineCtx::Fence { lang: None });
            }
        } else if let Some(lang) = &open {
            out.push(LineCtx::Fenced { lang: lang.clone() });
        } else {
            out.push(LineCtx::Text);
        }
    }
    // Unclosed fence at EOF — still reclass.
    if let Some(start) = body_start {
        reclass_fence_body_if_markdown(&mut out, start, lines);
    }
    out
}

/// Languages we always treat as markdown (explicit tags).
fn is_markdown_lang_tag(lang: &str) -> bool {
    matches!(
        lang.trim().to_ascii_lowercase().as_str(),
        "md" | "markdown" | "gfm" | "mdown" | "mkd"
    )
}

/// Unlabeled / plain tags are candidates for content-based markdown detection.
fn is_markdown_candidate_lang(lang: &str) -> bool {
    let l = lang.trim().to_ascii_lowercase();
    l.is_empty() || matches!(l.as_str(), "text" | "plain" | "plaintext" | "txt")
}

/// Rewrite `out[body_start..]` Fenced lines to `lang = "markdown"` when the
/// body scores as prose markdown (headings, bold, lists, tables, quotes).
fn reclass_fence_body_if_markdown(out: &mut [LineCtx], body_start: usize, lines: &[String]) {
    if body_start >= out.len() {
        return;
    }
    // Only reclass if the fence opened as a candidate (empty/text/plain).
    let Some(LineCtx::Fenced { lang }) = out.get(body_start) else {
        // Body may be empty (open+close with nothing between) or start at fence.
        return;
    };
    if !is_markdown_candidate_lang(lang) {
        return;
    }
    // Collect body text from matching source lines. `out[i]` corresponds to
    // `lines[i]` 1:1.
    let mut body: Vec<&str> = Vec::new();
    for (i, ctx) in out.iter().enumerate().skip(body_start) {
        match ctx {
            LineCtx::Fenced { .. } => {
                if let Some(line) = lines.get(i) {
                    body.push(line.as_str());
                }
            }
            _ => break,
        }
    }
    if !fence_body_looks_like_markdown(&body) {
        return;
    }
    for ctx in out.iter_mut().skip(body_start) {
        match ctx {
            LineCtx::Fenced { lang } => *lang = "markdown".into(),
            LineCtx::Fence { .. } => break, // hit the closer
            _ => break,
        }
    }
}

/// Heuristic: does this unlabeled fence body look like markdown prose rather
/// than a short code snippet?
///
/// Agent transcripts often open a bare ```` ``` ```` for a log line and then
/// keep writing headings/bold/lists without closing — those bodies must paint
/// as prose. Real unlabeled code fences (a few plain lines) score low and stay
/// code-styled.
fn fence_body_looks_like_markdown(body: &[&str]) -> bool {
    let mut score = 0i32;
    let mut non_empty = 0usize;
    let mut table_lines = 0usize;
    for line in body {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        non_empty += 1;
        if is_heading_line(t) {
            score += 3;
        }
        if is_list_line(t) {
            score += 2;
        }
        if is_quote_line(t) {
            score += 2;
        }
        if is_md_table_line(t) {
            table_lines += 1;
            score += 1;
        }
        // Inline markers common in agent prose.
        if t.contains("**") || t.contains("`]") {
            score += 1;
        }
        if t.contains('`') && t.matches('`').count() >= 2 {
            score += 1;
        }
        // Numbered summary sections without ATX: "1. " already covered by list.
    }
    if table_lines >= 2 {
        return true;
    }
    // Need real signal — a single bare `**` in one code comment shouldn't flip.
    // Long bodies with multiple md cues (typical agent dumps) flip easily.
    if non_empty >= 4 {
        score >= 3
    } else {
        score >= 4
    }
}

/// One display line produced by [`wrap_lines`]: a slice of an original logical
/// line, carrying the [`LineCtx`] it must be styled under. Continuation segments
/// (everything after the first) keep their line's ctx so fenced syntax accents
/// carry across the wrap.
///
/// Markdown table blocks are expanded at wrap time into column-aligned visual
/// rows (juice.ai discuss); those carry [`DisplayLine::md_roles`] so paint does
/// not re-split the laid-out text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayLine {
    pub text: String,
    pub ctx: LineCtx,
    /// `false` for the first segment of a logical line, `true` for the rest.
    pub is_continuation: bool,
    /// Pre-parsed juice.ai-style roles for a laid-out table visual row. When
    /// `Some`, [`style_display_line`] paints these instead of re-tokenizing
    /// `text` (so column padding and gutters survive).
    md_roles: Option<Vec<(String, SpanRole)>>,
}

impl DisplayLine {
    fn plain(text: String, ctx: LineCtx, is_continuation: bool) -> Self {
        Self {
            text,
            ctx,
            is_continuation,
            md_roles: None,
        }
    }

    fn table_row(roles: Vec<(String, SpanRole)>) -> Self {
        let text: String = roles.iter().map(|(s, _)| s.as_str()).collect();
        Self {
            text,
            ctx: LineCtx::Text,
            is_continuation: false,
            md_roles: Some(roles),
        }
    }
}

/// Cell width of `s` (unicode-width, matching ratatui's own layout — control
/// chars count 0 as they do in the render buffer).
fn str_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Cell width of one char (`None` — control chars — treated as 0, as ratatui does).
fn char_width(c: char) -> usize {
    UnicodeWidthChar::width(c).unwrap_or(0)
}

/// Char index in `text` whose cell span covers terminal cell column `target`.
/// Walks unicode cell widths left to right; a multi-width char covers all its
/// cells, so a click on either half maps to the same char. When `target` is at
/// or past the text's total cell width the index clamps to the last char (a
/// click in the trailing padding selects to end-of-line). Empty text → 0 (no
/// char; the caller treats an empty line as contributing no text). Used by the
/// detail-pane mouse selection to turn a cell column into a char boundary only
/// at the extraction edge, so the rest of the code stays in cell space.
pub fn char_at_cell(text: &str, target: usize) -> usize {
    let mut col = 0usize;
    let mut idx = 0usize;
    for (i, ch) in text.chars().enumerate() {
        idx = i;
        let w = char_width(ch);
        if w > 0 && target < col + w {
            return i;
        }
        col += w;
    }
    idx
}

/// Substring of `text` covered by the inclusive cell range `[lo, hi]`. Both cell
/// columns are mapped to char indices via [`char_at_cell`] and the inclusive
/// char slice is returned; `hi == usize::MAX` (the whole-line sentinel) selects
/// through the last char. Robust to `lo > hi` (clamps to empty-safe order) so a
/// caller that clamped absolute line indices can't trigger an underflow. Empty
/// text → "".
pub fn slice_cells(text: &str, lo: usize, hi: usize) -> String {
    let n = text.chars().count();
    if n == 0 {
        return String::new();
    }
    let hi_c = char_at_cell(text, hi).min(n - 1);
    let lo_c = char_at_cell(text, lo).min(hi_c);
    text.chars().skip(lo_c).take(hi_c + 1 - lo_c).collect()
}

/// Reflow logical lines into DISPLAY lines that each fit `width` cells, so every
/// consumer (scroll ceiling, windowing, scrollbar) agrees on the on-screen line
/// count. Called once per frame before windowing ([`crate::view::detail`]).
///
/// Rules:
/// - Fence RULE lines ([`LineCtx::Fence`]) never wrap — [`style_transcript_line`]
///   generates them at exactly `width`; they pass through as one segment.
/// - Empty logical lines stay one empty display line.
/// - A line already within `width` passes through unchanged (byte-for-byte,
///   indentation preserved) — so exact-width lines never spuriously wrap.
/// - Fenced code lines hard-break at the cell boundary (preserving every char,
///   including indentation); each continuation keeps the block's `Fenced` ctx.
/// - Text lines word-wrap at spaces; a single token wider than `width` (URLs!)
///   hard-breaks. Continuations are flush-left.
/// - Consecutive GFM table rows (`| … |`) in prose ([`LineCtx::Text`] or a
///   fenced markdown block) are laid out as a Grok full-grid table.
pub fn wrap_lines(lines: &[String], ctxs: &[LineCtx], width: usize) -> Vec<DisplayLine> {
    let width = width.max(1);
    let mut out = Vec::with_capacity(lines.len());
    let mut i = 0usize;
    while i < lines.len() {
        let line = &lines[i];
        let ctx = ctxs.get(i).cloned().unwrap_or(LineCtx::Text);

        // Markdown table block: group consecutive prose/md-fence table lines.
        if is_prose_ctx(&ctx) && is_md_table_line(line) {
            let start = i;
            i += 1;
            while i < lines.len()
                && ctxs.get(i).map(is_prose_ctx).unwrap_or(true)
                && is_md_table_line(&lines[i])
            {
                i += 1;
            }
            let block = &lines[start..i];
            if block.len() >= 2 {
                out.extend(layout_markdown_table(block, width));
                continue;
            }
            // Lone pipe-ish line — fall through as ordinary text (i already past
            // it; reprocess that single line below).
            i = start;
        }

        // Fence delimiters, lane-task rows (self-truncating in the styler), and
        // already-fitting/empty lines pass through as one segment. `str_width("")
        // == 0 <= width` folds the empty case in here.
        if matches!(
            ctx,
            LineCtx::Fence { .. } | LineCtx::LaneTask { .. } | LineCtx::LaneHeader
        ) || str_width(line) <= width
        {
            out.push(DisplayLine::plain(line.clone(), ctx, false));
            i += 1;
            continue;
        }
        // Markdown fences word-wrap like prose; real code fences hard-break.
        let pieces = match &ctx {
            LineCtx::Fenced { lang } if is_markdown_lang_tag(lang) => word_wrap(line, width),
            LineCtx::Fenced { .. } => hard_break(line, width),
            _ => word_wrap(line, width),
        };
        for (j, text) in pieces.into_iter().enumerate() {
            // A wrapped `key   value` row only has a key on its FIRST segment; the
            // continuation is pure value (word-wrap drops the alignment padding
            // too), so it must NOT re-color its first `key_col` chars as a key —
            // `key_col: 0` means "value column starts at 0". Every other ctx keeps
            // its styling across the wrap (fenced syntax, plain text).
            let seg_ctx = match &ctx {
                LineCtx::Config { .. } if j > 0 => LineCtx::Config { key_col: 0 },
                other => other.clone(),
            };
            out.push(DisplayLine::plain(text, seg_ctx, j > 0));
        }
        i += 1;
    }
    out
}

/// Style a display segment: prefers precomputed table roles when present.
pub fn style_display_line(seg: &DisplayLine, width: u16, p: &Palette) -> Line<'static> {
    if let Some(roles) = &seg.md_roles {
        return apply_jinja(spans_from_roles(roles.clone(), p), p);
    }
    if seg.text.is_empty() {
        return Line::from(" ");
    }
    style_transcript_line(&seg.text, &seg.ctx, width, p)
}

/// Split `line` into pieces each at most `width` cells, breaking at cell
/// boundaries (never mid-char). Every char is preserved. A single char wider
/// than `width` sits alone on its own piece.
fn hard_break(line: &str, width: usize) -> Vec<String> {
    let mut segs = Vec::new();
    let mut cur = String::new();
    let mut col = 0usize;
    for ch in line.chars() {
        let w = char_width(ch);
        if col + w > width && !cur.is_empty() {
            segs.push(std::mem::take(&mut cur));
            col = 0;
        }
        cur.push(ch);
        col += w;
    }
    if !cur.is_empty() || segs.is_empty() {
        segs.push(cur);
    }
    segs
}

/// Greedy word-wrap `line` to `width` cells. Breaks at whitespace; a single word
/// wider than `width` is hard-broken. The whitespace at a break point is dropped
/// (continuations are flush-left); leading indentation is kept on the first
/// segment.
fn word_wrap(line: &str, width: usize) -> Vec<String> {
    let indent_end = line.find(|c: char| !c.is_whitespace()).unwrap_or(line.len());
    let indent = &line[..indent_end];

    let mut segs: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut col = 0usize;
    let mut first = true;

    for word in line[indent_end..].split_whitespace() {
        let ww = str_width(word);
        if cur.is_empty() {
            // Fresh segment: prepend the original indent only on the first one.
            let prefix = if first { indent } else { "" };
            let candidate = format!("{prefix}{word}");
            if str_width(&candidate) <= width {
                cur = candidate;
                col = str_width(&cur);
            } else {
                col = push_hard_broken(&candidate, width, &mut segs, &mut cur);
            }
        } else if col + 1 + ww <= width {
            cur.push(' ');
            cur.push_str(word);
            col += 1 + ww;
        } else {
            segs.push(std::mem::take(&mut cur));
            if ww <= width {
                cur.push_str(word);
                col = ww;
            } else {
                col = push_hard_broken(word, width, &mut segs, &mut cur);
            }
        }
        first = false;
    }
    if !cur.is_empty() || segs.is_empty() {
        segs.push(cur);
    }
    segs
}

/// Hard-break `word`, pushing all but the last piece to `segs` and leaving the
/// last piece as the new current segment in `cur`. Returns the cell width of `cur`.
fn push_hard_broken(word: &str, width: usize, segs: &mut Vec<String>, cur: &mut String) -> usize {
    let mut pieces = hard_break(word, width);
    let last = pieces.pop().unwrap_or_default();
    segs.extend(pieces);
    let col = str_width(&last);
    *cur = last;
    col
}

/// Style a transcript line given its precomputed [`LineCtx`]. Fence delimiters
/// become horizontal rules (labeled with the language when opening); fenced
/// content gets best-effort, line-local syntax accents; plain text delegates to
/// [`style_line`] (juice.ai discuss/chat markdown: markers stripped, violet
/// headings / mint code / gold emphasis). `width` is the content width the rules
/// are sized to (any overflow is clipped by the `Paragraph`).
///
/// Rule precedence: fence-delimiter RULES are pure chrome. For every other line
/// a `{{jinja}}` overlay ([`apply_jinja`]) is applied LAST so placeholders stay
/// warn-yellow over mint code / gold bold. See [`style_line`] for prose roles.
pub fn style_transcript_line(line: &str, ctx: &LineCtx, width: u16, p: &Palette) -> Line<'static> {
    match ctx {
        LineCtx::Text => apply_jinja(style_line(line, p), p),
        LineCtx::Header => {
            Line::from(Span::styled(line.to_string(), Style::default().fg(p.heading).add_modifier(Modifier::BOLD)))
        }
        LineCtx::Fence { lang } => fence_rule(lang.as_deref(), width, p),
        LineCtx::Fenced { lang } => apply_jinja(style_fenced(line, lang, p), p),
        LineCtx::Config { key_col } => style_config_line(line, *key_col, p),
        LineCtx::LaneTask { glyph, is_def, created, age, live, selected } => {
            style_lane_task_line(line, *glyph, *is_def, created, age, live, *selected, width, p)
        }
        LineCtx::LaneHeader => style_lane_header_line(width, p),
    }
}

/// Style a queue-style lane-task row (see [`LineCtx::LaneTask`]): a status glyph
/// (colored by [`crate::view::theme::glyph_style`]), the task NAME (`line`) in
/// mauve for a definition or default grey for a prompt summary, then the fixed right-aligned
/// `created` / `age` columns (both `info` teal, like the queue pane's timestamp
/// and age) and the `live` column (warn, the "now" slot — `⏱ <elapsed>` for a
/// running task, `#N in lane` for a queued one, blank otherwise). Columns fit
/// `width` via [`crate::selectors::lane_task_cols`], degrading trailing columns
/// before the name so nothing is pushed off-screen. When `selected` the whole row
/// inverts with the palette selection style — the detail row cursor. Every char
/// lands in exactly one contiguous span so the cell-column selection patch keeps
/// working. `width == 0` yields an empty line.
#[allow(clippy::too_many_arguments)]
fn style_lane_task_line(
    name: &str,
    glyph: char,
    is_def: bool,
    created: &str,
    age: &str,
    live: &str,
    selected: bool,
    width: u16,
    p: &Palette,
) -> Line<'static> {
    let width = width as usize;
    if width == 0 {
        return Line::from(String::new());
    }
    let cols = crate::selectors::lane_task_cols(width);
    // Def name in the name color (mauve); a prompt summary in the terminal-default
    // grey (white is reserved for actions/tabs).
    let name_style = if is_def { Style::default().fg(p.mauve) } else { Style::default() };
    let gap = " ".repeat(crate::selectors::COL_GAP);
    let mut spans: Vec<Span<'static>> = vec![
        Span::styled(glyph.to_string(), crate::view::theme::glyph_style(glyph, p)),
        Span::raw(" "),
    ];
    if cols.name_w > 0 {
        spans.push(Span::styled(crate::selectors::pad_clip(name, cols.name_w), name_style));
    }
    if cols.created_w > 0 {
        spans.push(Span::raw(gap.clone()));
        spans.push(Span::styled(
            crate::selectors::pad_clip(created, cols.created_w),
            Style::default().fg(p.info),
        ));
    }
    if cols.age_w > 0 {
        spans.push(Span::raw(gap.clone()));
        spans.push(Span::styled(
            crate::selectors::pad_clip(age, cols.age_w),
            Style::default().fg(p.info),
        ));
    }
    if cols.live_w > 0 {
        spans.push(Span::raw(gap));
        // Blank live cells render as raw padding so the reserved column stays
        // aligned but reads as absent (not a warn-colored empty run).
        let style = if live.is_empty() { Style::default() } else { Style::default().fg(p.warn) };
        spans.push(Span::styled(crate::selectors::pad_clip(live, cols.live_w), style));
    }
    if selected {
        let sel = p.selection();
        for span in spans.iter_mut() {
            span.style = span.style.patch(sel);
        }
    }
    Line::from(spans)
}

/// Style the lane-task list's column-header row (see [`LineCtx::LaneHeader`]):
/// `Task Created Age Live` in the pane's de-emphasis dim, laid out with the SAME
/// [`crate::selectors::lane_task_cols`] widths as [`style_lane_task_line`] so the
/// labels sit over their columns (no label over the leading glyph slot). Never
/// selected-style — it is chrome, not a cursor row. `width == 0` yields an empty
/// line.
fn style_lane_header_line(width: u16, p: &Palette) -> Line<'static> {
    let width = width as usize;
    if width == 0 {
        return Line::from(String::new());
    }
    let cols = crate::selectors::lane_task_cols(width);
    let gap = " ".repeat(crate::selectors::COL_GAP);
    let dim = p.dim_style();
    // Two-cell glyph slot (glyph + space), no header text over it.
    let mut spans: Vec<Span<'static>> = vec![Span::raw("  ")];
    if cols.name_w > 0 {
        spans.push(Span::styled(crate::selectors::pad_clip("Task", cols.name_w), dim));
    }
    if cols.created_w > 0 {
        spans.push(Span::raw(gap.clone()));
        spans.push(Span::styled(crate::selectors::pad_clip("Created", cols.created_w), dim));
    }
    if cols.age_w > 0 {
        spans.push(Span::raw(gap.clone()));
        spans.push(Span::styled(crate::selectors::pad_clip("Age", cols.age_w), dim));
    }
    if cols.live_w > 0 {
        spans.push(Span::raw(gap));
        spans.push(Span::styled(crate::selectors::pad_clip("Live", cols.live_w), dim));
    }
    Line::from(spans)
}

/// Style a `key   value` config row: the key column (chars `[0, key_col)`,
/// including its right-padding) in `accent`, the value in `fg`. A lone `—`
/// placeholder value is dimmed; a value carrying a ` → ` resolution arrow dims
/// the arrow and emphasizes (bold `fg`) the resolved right-hand side. Splits at
/// the `key_col` CHAR boundary — the key + padding are always ASCII, so this is
/// also the cell boundary. A too-short line (no value column) styles wholly as a
/// key. Pure over the input; every char lands in exactly one contiguous span, so
/// the downstream cell-column selection patch keeps working.
fn style_config_line(line: &str, key_col: usize, p: &Palette) -> Line<'static> {
    let key_style = Style::default().fg(p.accent);
    // `key_col == 0` is a wrapped continuation (no key column) — style it wholly
    // as value so a wrapped path/value never mis-colors its start as a key.
    if key_col == 0 {
        return Line::from(style_config_value(line, Style::default(), p));
    }
    let chars: Vec<char> = line.chars().collect();
    if chars.len() <= key_col {
        return Line::from(Span::styled(line.to_string(), key_style));
    }
    let key: String = chars[..key_col].iter().collect();
    let value: String = chars[key_col..].iter().collect();
    let base = config_value_style(key.trim(), p);
    let mut spans = vec![Span::styled(key, key_style)];
    spans.extend(style_config_value(&value, base, p));
    Line::from(spans)
}

/// Base color for a config VALUE, keyed on its (trimmed) key so the same concept
/// reads in the same color as the panes: timestamps in teal, `pr`/`model` in the
/// metadata gold, everything else the terminal-default grey (white is reserved
/// for actions/tabs). `pr` also underlines (link affordance).
fn config_value_style(key: &str, p: &Palette) -> Style {
    match key {
        "created" | "started" | "finished" | "updated" => Style::default().fg(p.info),
        "pr" => Style::default().fg(p.meta).add_modifier(Modifier::UNDERLINED),
        "model" => Style::default().fg(p.meta),
        _ => Style::default(), // default grey — the terminal's own foreground
    }
}

/// Spans for a config row's value column (see [`style_config_line`]). `base` is
/// the concept color from [`config_value_style`]; a wrapped continuation passes
/// the default grey.
fn style_config_value(value: &str, base: Style, p: &Palette) -> Vec<Span<'static>> {
    if value.trim() == "—" {
        return vec![Span::styled(value.to_string(), p.dim_style())];
    }
    // Fallback-chain display (`a → b → c`): every label uses the concept color
    // equally; arrows stay dim. No bold "current head" — the chain is ordered
    // walk order, not a remap of old → new.
    if value.contains(" → ") {
        let mut spans = Vec::new();
        let mut first = true;
        for part in value.split(" → ") {
            if !first {
                spans.push(Span::styled(" → ".to_string(), p.dim_style()));
            }
            first = false;
            spans.push(Span::styled(part.to_string(), base));
        }
        return spans;
    }
    vec![Span::styled(value.to_string(), base)]
}

/// Char-index ranges `[start, end)` of every `{{...}}` placeholder in `s`. The
/// nearest `}}` closes each `{{` (non-greedy); a `{{` with no closing `}}` on the
/// line yields no range (matching per-line styling — a lone `{{` stays unstyled).
/// The braces are included in the range so the whole placeholder is highlighted.
fn jinja_ranges(s: &str) -> Vec<(usize, usize)> {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut ranges = Vec::new();
    let mut i = 0usize;
    while i + 1 < n {
        if chars[i] == '{' && chars[i + 1] == '{' {
            let mut j = i + 2;
            let mut close = None;
            while j + 1 < n {
                if chars[j] == '}' && chars[j + 1] == '}' {
                    close = Some(j + 2);
                    break;
                }
                j += 1;
            }
            match close {
                Some(end) => {
                    ranges.push((i, end));
                    i = end;
                    continue;
                }
                None => break, // no closing `}}` on this line
            }
        }
        i += 1;
    }
    ranges
}

/// Overlay `{{jinja}}` warn-yellow styling onto an already-styled `line`, keying
/// on char ranges found over the concatenated text ([`jinja_ranges`]) and
/// splitting spans at the range boundaries so surrounding syntax colors survive.
/// Placeholder chars get a flat warn fg (replacing any base color, so inline-code
/// green becomes yellow). No-op when the line has no placeholder. Pure; span
/// shape stays well-formed (every char in exactly one contiguous span) so the
/// downstream cell-column selection patch keeps working.
fn apply_jinja(line: Line<'static>, p: &Palette) -> Line<'static> {
    let full: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    let ranges = jinja_ranges(&full);
    if ranges.is_empty() {
        return line;
    }
    let warn = Style::default().fg(p.warn);
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut idx = 0usize; // global char index across the whole line
    for span in &line.spans {
        let base = span.style;
        let mut buf = String::new();
        let mut buf_jinja = false;
        for ch in span.content.chars() {
            let in_jinja = ranges.iter().any(|&(a, b)| idx >= a && idx < b);
            if !buf.is_empty() && in_jinja != buf_jinja {
                out.push(Span::styled(std::mem::take(&mut buf), if buf_jinja { warn } else { base }));
            }
            buf.push(ch);
            buf_jinja = in_jinja;
            idx += 1;
        }
        if !buf.is_empty() {
            out.push(Span::styled(buf, if buf_jinja { warn } else { base }));
        }
    }
    Line::from(out)
}

/// A horizontal rule sized to `width`. With a language it embeds the label as
/// `──────── lang ───────` (rule chars in `p.border`, label in the `p.dim`
/// de-emphasis grey); without one it is a plain full-width rule.
fn fence_rule(lang: Option<&str>, width: u16, p: &Palette) -> Line<'static> {
    let width = width as usize;
    let rule = Style::default().fg(p.border);
    match lang {
        Some(lang) if !lang.is_empty() => {
            let label_w = lang.chars().count() + 2; // a space either side of the label
            let trailing = width
                .saturating_sub(FENCE_RULE_PREFIX + label_w)
                .max(FENCE_RULE_MIN_TRAIL);
            Line::from(vec![
                Span::styled(rule_run(FENCE_RULE_PREFIX), rule),
                Span::styled(format!(" {lang} "), p.dim_style()),
                Span::styled(rule_run(trailing), rule),
            ])
        }
        _ => Line::from(Span::styled(rule_run(width.max(FENCE_RULE_MIN_TRAIL)), rule)),
    }
}

fn rule_run(n: usize) -> String {
    RULE_CHAR.to_string().repeat(n)
}

enum FenceLang {
    Markdown,
    Bash,
    Json,
    /// Generic source (rust/python/…) — light string/comment accents.
    Code,
}

impl FenceLang {
    fn classify(lang: &str) -> Self {
        let l = lang.trim().to_ascii_lowercase();
        if is_markdown_lang_tag(&l) {
            return FenceLang::Markdown;
        }
        match l.as_str() {
            "bash" | "sh" | "shell" | "zsh" | "console" | "shellsession" => FenceLang::Bash,
            "json" | "jsonc" => FenceLang::Json,
            // Explicit plain — leave unstyled (brightness rule).
            "text" | "plain" | "plaintext" | "txt" | "" => FenceLang::Code,
            _ => FenceLang::Code,
        }
    }
}

/// Prose ctx for table grouping / word-wrap: free text or a reclassed markdown fence.
fn is_prose_ctx(ctx: &LineCtx) -> bool {
    match ctx {
        LineCtx::Text => true,
        LineCtx::Fenced { lang } => is_markdown_lang_tag(lang),
        _ => false,
    }
}

/// Dispatch fenced content: markdown fences use the prose styler (same as
/// [`style_line`]); bash/json keep their heuristics; everything else gets a
/// light generic code accent (strings + comments) so unlabeled dumps aren't flat.
fn style_fenced(line: &str, lang: &str, p: &Palette) -> Line<'static> {
    match FenceLang::classify(lang) {
        FenceLang::Markdown => style_line(line, p),
        FenceLang::Bash => style_bash(line, p),
        FenceLang::Json => style_json(line, p),
        FenceLang::Code => style_code_generic(line, p),
    }
}

/// Light line-local accents for generic source fences (no full grammar):
/// - `#` / `//` line comments → dim
/// - `"…"` / `'…'` strings → mint code
/// - remaining text plain
///
/// Enough to make python/rust/go dumps readable without syntect.
fn style_code_generic(line: &str, p: &Palette) -> Line<'static> {
    let code = Style::default().fg(MD_CODE);
    let dim = p.dim_style();
    let plain = Style::default();
    let t = line.trim_start();
    // Whole-line comments.
    if t.starts_with("//") || t.starts_with('#') || t.starts_with("-- ") || t == "--" {
        return Line::from(Span::styled(line.to_string(), dim));
    }

    let mut spans: Vec<Span<'static>> = Vec::new();
    let b = line.as_bytes();
    let n = b.len();
    let mut i = 0usize;
    let mut plain_buf = String::new();
    let flush_plain = |buf: &mut String, spans: &mut Vec<Span<'static>>| {
        if !buf.is_empty() {
            spans.push(Span::styled(std::mem::take(buf), plain));
        }
    };
    while i < n {
        // Line comment mid-line (`code // note`).
        if b[i] == b'/' && i + 1 < n && b[i + 1] == b'/' {
            flush_plain(&mut plain_buf, &mut spans);
            spans.push(Span::styled(line[i..].to_string(), dim));
            break;
        }
        if b[i] == b'#' && (i == 0 || b[i - 1].is_ascii_whitespace()) {
            flush_plain(&mut plain_buf, &mut spans);
            spans.push(Span::styled(line[i..].to_string(), dim));
            break;
        }
        // Quoted string.
        if b[i] == b'"' || b[i] == b'\'' {
            let quote = b[i];
            flush_plain(&mut plain_buf, &mut spans);
            let start = i;
            i += 1;
            while i < n {
                if b[i] == b'\\' && i + 1 < n {
                    i += 2;
                    continue;
                }
                if b[i] == quote {
                    i += 1;
                    break;
                }
                i += 1;
            }
            spans.push(Span::styled(line[start..i].to_string(), code));
            continue;
        }
        let ch = line[i..].chars().next().unwrap();
        plain_buf.push(ch);
        i += ch.len_utf8();
    }
    flush_plain(&mut plain_buf, &mut spans);
    if spans.is_empty() {
        spans.push(Span::raw(line.to_string()));
    }
    Line::from(spans)
}

/// bash accents (line-local heuristic, no shell parser): the first token of the
/// line and the first token after each `&&`/`||`/`|`/`;` separator → green;
/// quoted spans → yellow; tokens starting with `/`, `~/`, `./` → blue; else
/// default fg. Command position wins over the path rule (a `./script` command
/// stays green).
fn style_bash(line: &str, p: &Palette) -> Line<'static> {
    let ok = Style::default().fg(p.ok);
    let warn = Style::default().fg(p.warn);
    let accent = Style::default().fg(p.accent);
    let plain = Style::default();

    let mut spans: Vec<Span<'static>> = Vec::new();
    let b = line.as_bytes();
    let n = b.len();
    let mut i = 0usize;
    let mut cmd_pos = true;
    while i < n {
        let c = b[i];
        if c.is_ascii_whitespace() {
            let start = i;
            while i < n && b[i].is_ascii_whitespace() {
                i += 1;
            }
            spans.push(Span::styled(line[start..i].to_string(), plain));
        } else if c == b'\'' || c == b'"' {
            let start = i;
            i += 1;
            while i < n && b[i] != c {
                i += 1;
            }
            if i < n {
                i += 1; // include the closing quote
            }
            spans.push(Span::styled(line[start..i].to_string(), warn));
            cmd_pos = false;
        } else if matches!(c, b'&' | b'|' | b';') {
            let start = i;
            if (c == b'&' && i + 1 < n && b[i + 1] == b'&')
                || (c == b'|' && i + 1 < n && b[i + 1] == b'|')
            {
                i += 2;
            } else {
                i += 1;
            }
            spans.push(Span::styled(line[start..i].to_string(), plain));
            cmd_pos = true;
        } else {
            let start = i;
            while i < n && !b[i].is_ascii_whitespace() && !matches!(b[i], b'\'' | b'"' | b'&' | b'|' | b';') {
                i += 1;
            }
            let word = &line[start..i];
            let style = if cmd_pos {
                ok
            } else if word.starts_with('/') || word.starts_with("~/") || word.starts_with("./") {
                accent
            } else {
                plain
            };
            spans.push(Span::styled(word.to_string(), style));
            cmd_pos = false;
        }
    }
    if spans.is_empty() {
        spans.push(Span::raw(line.to_string()));
    }
    Line::from(spans)
}

/// json accents (line-local heuristic): `"key":` keys → accent; other quoted
/// strings → green; numbers/`true`/`false`/`null` → mauve; structural chars →
/// default fg.
fn style_json(line: &str, p: &Palette) -> Line<'static> {
    let accent = Style::default().fg(p.accent);
    let ok = Style::default().fg(p.ok);
    let mauve = Style::default().fg(p.mauve);
    let plain = Style::default();

    let mut spans: Vec<Span<'static>> = Vec::new();
    let b = line.as_bytes();
    let n = b.len();
    let mut i = 0usize;
    while i < n {
        let c = b[i];
        if c == b'"' {
            let start = i;
            i += 1;
            while i < n {
                if b[i] == b'\\' && i + 1 < n {
                    i += 2;
                    continue;
                }
                if b[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            // A `:` after optional whitespace makes this a key.
            let mut j = i;
            while j < n && b[j].is_ascii_whitespace() {
                j += 1;
            }
            let key = j < n && b[j] == b':';
            spans.push(Span::styled(line[start..i].to_string(), if key { accent } else { ok }));
        } else if c == b'-' || c.is_ascii_digit() {
            let start = i;
            i += 1;
            while i < n && (b[i].is_ascii_digit() || matches!(b[i], b'.' | b'e' | b'E' | b'+' | b'-')) {
                i += 1;
            }
            spans.push(Span::styled(line[start..i].to_string(), mauve));
        } else if let Some(lit) = json_literal_at(&line[i..]) {
            let end = i + lit.len();
            spans.push(Span::styled(line[i..end].to_string(), mauve));
            i = end;
        } else {
            // Advance whole chars: a byte-wise step from a multi-byte char's
            // leading byte would land mid-char and panic on `&line[i..]` below.
            let start = i;
            i += line[i..].chars().next().map_or(1, char::len_utf8);
            while i < n
                && b[i] != b'"'
                && b[i] != b'-'
                && !b[i].is_ascii_digit()
                && json_literal_at(&line[i..]).is_none()
            {
                i += line[i..].chars().next().map_or(1, char::len_utf8);
            }
            spans.push(Span::styled(line[start..i].to_string(), plain));
        }
    }
    if spans.is_empty() {
        spans.push(Span::raw(line.to_string()));
    }
    Line::from(spans)
}

/// `true`/`false`/`null` at the start of `rest`, only when the literal isn't
/// glued to a following word char (so `nullable` is not read as `null`).
fn json_literal_at(rest: &str) -> Option<&'static str> {
    for lit in ["true", "false", "null"] {
        if let Some(after) = rest.strip_prefix(lit)
            && !after.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Some(lit);
        }
    }
    None
}

// juice.ai discuss/chat markdown roles (`ColorProfile` fg_heading / fg_code /
// fg_emph). Fixed so transcript prose matches the chat view regardless of which
// qoo chrome theme is active.
const MD_HEADING: Color = Color::Rgb(214, 158, 255); // #d69eff bright soft violet
const MD_CODE: Color = Color::Rgb(126, 231, 168); // #7ee7a8 soft mint
const MD_EMPH: Color = Color::Rgb(232, 201, 138); // #e8c98a soft gold (bold + italic)

/// Inline role after markdown markers are stripped (drives span paint).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpanRole {
    Body,
    Bold,
    Italic,
    Code,
    Dim,
    Heading,
    /// URL — accent blue (queohoh extra; juice.ai chat has no URL role).
    Link,
}

fn style_for_role(role: SpanRole, p: &Palette) -> Style {
    match role {
        SpanRole::Body => Style::default(),
        SpanRole::Bold => Style::default().fg(MD_EMPH).add_modifier(Modifier::BOLD),
        SpanRole::Italic => Style::default().fg(MD_EMPH).add_modifier(Modifier::ITALIC),
        SpanRole::Code => Style::default().fg(MD_CODE),
        SpanRole::Dim => p.dim_style(),
        SpanRole::Heading => Style::default().fg(MD_HEADING).add_modifier(Modifier::BOLD),
        SpanRole::Link => Style::default().fg(p.accent),
    }
}

fn spans_from_roles(roles: Vec<(String, SpanRole)>, p: &Palette) -> Line<'static> {
    if roles.is_empty() {
        return Line::from(Span::raw(String::new()));
    }
    Line::from(
        roles
            .into_iter()
            .map(|(t, r)| Span::styled(t, style_for_role(r, p)))
            .collect::<Vec<_>>(),
    )
}

/// Style one detail-pane prose line — port of juice.ai `discuss` chat markdown
/// (`display_spans_for_line` + `parse_inline`). Markers are stripped; headings
/// are violet, emphasis gold, inline code mint; lists use a dim `• `; GFM table
/// rows paint dim gutters. The `{{jinja}}` overlay is applied by the caller
/// ([`style_transcript_line`]), not here.
pub fn style_line(line: &str, p: &Palette) -> Line<'static> {
    if line.is_empty() {
        return Line::from(Span::raw(String::new()));
    }
    if is_rule(line) {
        // Border color — matches fenced-block rules.
        return Line::from(Span::styled(line.to_string(), Style::default().fg(p.border)));
    }
    if is_heading_line(line.trim_start()) {
        let body = strip_heading_marker(line);
        let roles = parse_inline(body)
            .into_iter()
            .map(|(s, role)| {
                let r = match role {
                    SpanRole::Code | SpanRole::Link => role,
                    _ => SpanRole::Heading,
                };
                (s, r)
            })
            .collect();
        return spans_from_roles(roles, p);
    }
    if is_md_table_line(line) {
        return spans_from_roles(style_table_row_roles(line), p);
    }
    if is_quote_line(line.trim_start()) {
        return spans_from_roles(style_quote_roles(line), p);
    }
    if is_list_line(line.trim_start()) {
        return spans_from_roles(style_list_roles(line), p);
    }
    spans_from_roles(parse_inline(line), p)
}

fn style_list_roles(line: &str) -> Vec<(String, SpanRole)> {
    let t = line.trim_start();
    let lead_n = line.len() - t.len();
    let lead = " ".repeat(lead_n);
    let body = strip_list_marker(t);
    let mut spans = Vec::new();
    if lead_n > 0 {
        spans.push((lead, SpanRole::Body));
    }
    spans.push(("• ".into(), SpanRole::Dim));
    spans.extend(parse_inline(&body));
    spans
}

fn style_quote_roles(line: &str) -> Vec<(String, SpanRole)> {
    let t = line.trim_start();
    let lead_n = line.len() - t.len();
    let lead = " ".repeat(lead_n);
    let body = t
        .strip_prefix("> ")
        .or_else(|| t.strip_prefix('>'))
        .unwrap_or(t);
    let mut spans = Vec::new();
    if lead_n > 0 {
        spans.push((lead, SpanRole::Dim));
    }
    spans.push(("│ ".into(), SpanRole::Dim));
    for (s, role) in parse_inline(body) {
        let r = match role {
            SpanRole::Bold | SpanRole::Italic | SpanRole::Code | SpanRole::Link => role,
            _ => SpanRole::Dim,
        };
        spans.push((s, r));
    }
    spans
}

/// Style a lone GFM table line (block layout already handled in [`wrap_lines`]).
fn style_table_row_roles(line: &str) -> Vec<(String, SpanRole)> {
    if is_md_table_sep(line) {
        let cells = split_table_cells(line.trim());
        let mut spans = Vec::new();
        for (c, cell) in cells.iter().enumerate() {
            if c > 0 {
                spans.push(("─┼─".into(), SpanRole::Dim));
            }
            let w = cell.chars().filter(|ch| *ch == '-').count().max(3);
            spans.push(("─".repeat(w), SpanRole::Dim));
        }
        if spans.is_empty() {
            spans.push(("───".into(), SpanRole::Dim));
        }
        return spans;
    }
    let cells = split_table_cells(line.trim());
    if cells.is_empty() {
        return parse_inline(line);
    }
    let mut spans = Vec::new();
    for (c, cell) in cells.iter().enumerate() {
        if c > 0 {
            spans.push((" │ ".into(), SpanRole::Dim));
        }
        let cell_spans = parse_inline(cell);
        if cell_spans.is_empty() {
            spans.push((String::new(), SpanRole::Body));
        } else {
            spans.extend(cell_spans);
        }
    }
    spans
}

/// Lay out a GFM table block as a **Grok-style full grid**: outer border, per-row
/// and per-column rules (`┌─┬─┐` / `│ │` / `├─┼─┤` / `└─┴─┘`). Markdown
/// separator rows (`|---|`) only mark the header; they are not painted as content.
fn layout_markdown_table(block: &[String], width: usize) -> Vec<DisplayLine> {
    if width == 0 || block.is_empty() {
        return Vec::new();
    }

    let mut raw: Vec<(bool, Vec<String>)> = Vec::new(); // (is_sep, cells)
    for line in block {
        let cells = split_table_cells(line);
        if cells.iter().all(|c| c.is_empty()) {
            continue;
        }
        raw.push((is_md_table_sep(line), cells));
    }
    if raw.is_empty() {
        return Vec::new();
    }

    // Header = first row when the second is a GFM separator.
    let has_header = raw.len() >= 2 && raw[1].0;
    // Drop separator rows — borders replace them.
    let data_rows: Vec<Vec<String>> = raw
        .into_iter()
        .filter(|(is_sep, _)| !*is_sep)
        .map(|(_, cells)| cells)
        .collect();
    if data_rows.is_empty() {
        return Vec::new();
    }

    let ncols = data_rows.iter().map(|c| c.len()).max().unwrap_or(0).max(1);
    let mut data_rows = data_rows;
    for cells in &mut data_rows {
        cells.resize(ncols, String::new());
    }

    // Natural column widths (display width after stripping inline markers).
    // 1-char left+right padding inside each cell (Grok-like breathing room).
    const CELL_PAD: usize = 1; // space each side of content
    let mut col_w = vec![1usize; ncols];
    for cells in &data_rows {
        for (c, cell) in cells.iter().enumerate() {
            col_w[c] = col_w[c].max(table_cell_plain_width(cell).max(1));
        }
    }

    // Fit: content cols + vertical borders (ncols + 1) + 2*pad per column.
    // Line shape: `│` + pad + content*w + pad + `│` + … + `│`
    // total = sum(col_w) + ncols * (2 * CELL_PAD) + (ncols + 1)
    let border_budget = ncols + 1 + ncols * (2 * CELL_PAD);
    let avail = width.saturating_sub(border_budget).max(ncols);
    let mut total: usize = col_w.iter().sum();
    while total > avail {
        let Some((idx, _)) = col_w
            .iter()
            .enumerate()
            .filter(|(_, w)| **w > 1)
            .max_by_key(|(_, w)| *w)
        else {
            break;
        };
        col_w[idx] -= 1;
        total -= 1;
    }

    // Pre-wrap every data row's cells.
    let mut wrapped_rows: Vec<(bool, Vec<Vec<Vec<(String, SpanRole)>>>)> =
        Vec::with_capacity(data_rows.len());
    for (ri, cells) in data_rows.iter().enumerate() {
        let header_row = has_header && ri == 0;
        let mut wrapped_cells: Vec<Vec<Vec<(String, SpanRole)>>> = Vec::with_capacity(ncols);
        for (c, cell) in cells.iter().enumerate() {
            let mut cell_spans = parse_inline(cell);
            if header_row {
                // Grok headers: bold body (not violet) — code/links keep their roles.
                cell_spans = cell_spans
                    .into_iter()
                    .map(|(s, role)| {
                        let r = match role {
                            SpanRole::Code | SpanRole::Link => role,
                            _ => SpanRole::Bold,
                        };
                        (s, r)
                    })
                    .collect();
            }
            wrapped_cells.push(wrap_roles_to_width(&cell_spans, col_w[c].max(1)));
        }
        wrapped_rows.push((header_row, wrapped_cells));
    }

    let mut out = Vec::new();
    out.push(DisplayLine::table_row(box_rule(&col_w, BoxRule::Top)));

    for (ri, (_header_row, wrapped_cells)) in wrapped_rows.iter().enumerate() {
        let row_h = wrapped_cells
            .iter()
            .map(|lines| lines.len())
            .max()
            .unwrap_or(1)
            .max(1);

        for line_i in 0..row_h {
            let mut spans: Vec<(String, SpanRole)> = Vec::new();
            spans.push(("│".into(), SpanRole::Dim));
            for c in 0..ncols {
                // left pad
                spans.push((" ".repeat(CELL_PAD), SpanRole::Body));
                let w = col_w[c];
                let mut line_spans = wrapped_cells[c]
                    .get(line_i)
                    .cloned()
                    .unwrap_or_else(|| vec![(String::new(), SpanRole::Body)]);
                if line_spans.len() == 1 && line_spans[0].0.is_empty() {
                    line_spans = vec![(String::new(), SpanRole::Body)];
                }
                let pw = str_width(&roles_plain(&line_spans));
                if pw < w {
                    line_spans.push((" ".repeat(w - pw), SpanRole::Body));
                } else if pw > w {
                    // Safety: hard-clip plain width if wrap under-shot.
                    line_spans = clip_roles_to_width(line_spans, w);
                }
                spans.extend(line_spans);
                // right pad
                spans.push((" ".repeat(CELL_PAD), SpanRole::Body));
                spans.push(("│".into(), SpanRole::Dim));
            }
            out.push(DisplayLine::table_row(spans));
        }

        if ri + 1 < wrapped_rows.len() {
            out.push(DisplayLine::table_row(box_rule(&col_w, BoxRule::Mid)));
        }
    }

    out.push(DisplayLine::table_row(box_rule(&col_w, BoxRule::Bottom)));
    out
}

#[derive(Clone, Copy)]
enum BoxRule {
    Top,
    Mid,
    Bottom,
}

/// Horizontal rule for the Grok-style grid (`┌─┬─┐` / `├─┼─┤` / `└─┴─┘`).
/// Interior segment width = col_w + 2*pad (matches content lines).
fn box_rule(col_w: &[usize], kind: BoxRule) -> Vec<(String, SpanRole)> {
    const CELL_PAD: usize = 1;
    let (left, mid, right, fill) = match kind {
        BoxRule::Top => ('┌', '┬', '┐', '─'),
        BoxRule::Mid => ('├', '┼', '┤', '─'),
        BoxRule::Bottom => ('└', '┴', '┘', '─'),
    };
    let mut spans = Vec::new();
    spans.push((left.to_string(), SpanRole::Dim));
    for (c, &w) in col_w.iter().enumerate() {
        let seg = w + 2 * CELL_PAD;
        spans.push((fill.to_string().repeat(seg), SpanRole::Dim));
        spans.push((
            if c + 1 < col_w.len() {
                mid.to_string()
            } else {
                right.to_string()
            },
            SpanRole::Dim,
        ));
    }
    spans
}

/// Hard-clip role spans to `width` display cells (last resort).
fn clip_roles_to_width(
    spans: Vec<(String, SpanRole)>,
    width: usize,
) -> Vec<(String, SpanRole)> {
    if width == 0 {
        return vec![(String::new(), SpanRole::Body)];
    }
    let mut out = Vec::new();
    let mut used = 0usize;
    for (text, role) in spans {
        if used >= width {
            break;
        }
        let mut chunk = String::new();
        for ch in text.chars() {
            let cw = char_width(ch);
            if used + cw > width {
                break;
            }
            chunk.push(ch);
            used += cw;
        }
        if !chunk.is_empty() {
            out.push((chunk, role));
        }
    }
    if out.is_empty() {
        out.push((String::new(), SpanRole::Body));
    }
    // Pad if we clipped short (e.g. wide char refused).
    let pw = str_width(&roles_plain(&out));
    if pw < width {
        out.push((" ".repeat(width - pw), SpanRole::Body));
    }
    out
}

fn table_cell_plain_width(cell: &str) -> usize {
    str_width(&roles_plain(&parse_inline(cell)))
}

fn roles_plain(spans: &[(String, SpanRole)]) -> String {
    spans.iter().map(|(s, _)| s.as_str()).collect()
}

/// Word-wrap role spans to `width` cells (roles preserved across breaks).
fn wrap_roles_to_width(spans: &[(String, SpanRole)], width: usize) -> Vec<Vec<(String, SpanRole)>> {
    if width == 0 {
        return vec![vec![(String::new(), SpanRole::Body)]];
    }
    let plain = roles_plain(spans);
    if plain.is_empty() {
        return vec![spans.to_vec()];
    }
    if str_width(&plain) <= width {
        return vec![spans.to_vec()];
    }

    // Flatten to (char, role), then greedy word-wrap.
    let mut chars: Vec<(char, SpanRole)> = Vec::new();
    for (text, role) in spans {
        for ch in text.chars() {
            chars.push((ch, *role));
        }
    }

    let mut rows: Vec<Vec<(String, SpanRole)>> = Vec::new();
    let mut rest: &[(char, SpanRole)] = &chars;
    while !rest.is_empty() {
        let (line_chars, next) = take_wrapped_role_chars(rest, width);
        rest = next;
        rows.push(coalesce_role_chars(&line_chars));
    }
    if rows.is_empty() {
        rows.push(vec![(String::new(), SpanRole::Body)]);
    }
    rows
}

fn take_wrapped_role_chars(
    chars: &[(char, SpanRole)],
    max_cols: usize,
) -> (Vec<(char, SpanRole)>, &[(char, SpanRole)]) {
    if chars.is_empty() || max_cols == 0 {
        return (Vec::new(), chars);
    }
    let total_w: usize = chars.iter().map(|(ch, _)| char_width(*ch)).sum();
    if total_w <= max_cols {
        return (chars.to_vec(), &[]);
    }

    let mut cols = 0usize;
    let mut last_ws: Option<usize> = None;
    let mut end_idx = chars.len();
    for (i, (ch, _)) in chars.iter().enumerate() {
        let cw = char_width(*ch);
        if cols + cw > max_cols {
            end_idx = i;
            break;
        }
        if ch.is_whitespace() {
            last_ws = Some(i);
        }
        cols += cw;
    }

    if let Some(ws) = last_ws {
        if ws > 0 {
            let line = chars[..ws].to_vec();
            let mut rest = &chars[ws..];
            while let Some((c, _)) = rest.first() {
                if c.is_whitespace() {
                    rest = &rest[1..];
                } else {
                    break;
                }
            }
            return (line, rest);
        }
    }
    if end_idx == 0 {
        end_idx = 1; // force at least one char
    }
    (chars[..end_idx].to_vec(), &chars[end_idx..])
}

fn coalesce_role_chars(chars: &[(char, SpanRole)]) -> Vec<(String, SpanRole)> {
    let mut out: Vec<(String, SpanRole)> = Vec::new();
    for &(ch, role) in chars {
        if let Some((s, r)) = out.last_mut() {
            if *r == role {
                s.push(ch);
                continue;
            }
        }
        out.push((ch.to_string(), role));
    }
    if out.is_empty() {
        out.push((String::new(), SpanRole::Body));
    }
    out
}

/// True when a line looks like a GFM table row (`| a | b |` or `a | b`).
fn is_md_table_line(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() || !t.contains('|') {
        return false;
    }
    if t.chars().all(|c| c == '|' || c.is_whitespace()) {
        return true;
    }
    if t.starts_with('|') {
        return true;
    }
    split_table_cells(t).len() >= 2
}

/// Separator row: only dashes/colons/spaces in every cell (`|---|:---:|`).
fn is_md_table_sep(line: &str) -> bool {
    let cells = split_table_cells(line.trim());
    !cells.is_empty()
        && cells.iter().all(|c| {
            !c.is_empty()
                && c.chars().all(|ch| matches!(ch, '-' | ':' | ' '))
                && c.chars().any(|ch| ch == '-')
        })
}

fn split_table_cells(line: &str) -> Vec<String> {
    let t = line.trim();
    let mut s = t;
    if let Some(r) = s.strip_prefix('|') {
        s = r;
    }
    if let Some(r) = s.strip_suffix('|') {
        s = r;
    }
    s.split('|').map(|c| c.trim().to_string()).collect()
}

fn is_heading_line(t: &str) -> bool {
    let b = t.as_bytes();
    if b.is_empty() || b[0] != b'#' {
        return false;
    }
    let mut i = 0usize;
    while i < b.len() && b[i] == b'#' {
        i += 1;
    }
    // ATX headings: 1–6 hashes, then space or end.
    i >= 1 && i <= 6 && (i == b.len() || b[i] == b' ')
}

fn is_list_line(t: &str) -> bool {
    if t.starts_with("- ") || t.starts_with("* ") || t.starts_with("+ ") {
        return true;
    }
    // `*` alone as bullet (space required in juice; keep +/`- ` strict).
    let bytes = t.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    i > 0 && (t[i..].starts_with(". ") || t[i..].starts_with(") "))
}

fn is_quote_line(t: &str) -> bool {
    t.starts_with("> ") || t == ">"
}

/// Strip leading `#+\s*` from a heading line (display text only).
fn strip_heading_marker(line: &str) -> &str {
    let t = line.trim_start();
    let mut i = 0usize;
    let b = t.as_bytes();
    while i < b.len() && b[i] == b'#' {
        i += 1;
    }
    if i == 0 {
        return t;
    }
    if i < b.len() && b[i] == b' ' {
        i += 1;
    }
    &t[i..]
}

/// Strip `- ` / `* ` / `+ ` / `N. ` / `N) ` from a list line (already trim_start'd).
fn strip_list_marker(t: &str) -> String {
    if let Some(rest) = t
        .strip_prefix("- ")
        .or_else(|| t.strip_prefix("* "))
        .or_else(|| t.strip_prefix("+ "))
    {
        return rest.to_string();
    }
    let bytes = t.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i > 0 {
        if t[i..].starts_with(". ") {
            return t[i + 2..].to_string();
        }
        if t[i..].starts_with(") ") {
            return t[i + 2..].to_string();
        }
    }
    t.to_string()
}

/// Span-level parse of inline markdown. Markers are stripped.
///
/// Supports:
/// - `` `code` ``
/// - `**bold**` / `__bold__`
/// - `*italic*` / `_italic_`
/// - `http(s)://…` URLs (queohoh extra)
///
/// Bold/italic interiors are re-parsed so nested `` `code` `` still strips
/// (common LLM pattern: `` **`col` vs `other`** ``). Port of juice.ai
/// `discuss::parse_inline`.
fn parse_inline(s: &str) -> Vec<(String, SpanRole)> {
    let mut out: Vec<(String, SpanRole)> = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0usize;
    let mut plain = String::new();

    let flush_plain = |plain: &mut String, out: &mut Vec<(String, SpanRole)>| {
        if !plain.is_empty() {
            // Scan flushed plain for URLs so links still paint accent blue.
            out.extend(split_urls(std::mem::take(plain)));
        }
    };

    while i < s.len() {
        // Inline code: `...` (prefer before * so `*_usd` stays one code span).
        if bytes[i] == b'`' {
            if let Some(end) = s[i + 1..].find('`').map(|o| i + 1 + o) {
                flush_plain(&mut plain, &mut out);
                out.push((s[i + 1..end].to_string(), SpanRole::Code));
                i = end + 1;
                continue;
            }
            // Unmatched opener — drop the stray backtick rather than show it.
            i += 1;
            continue;
        }
        // Bold: **...**
        if bytes[i] == b'*' && i + 1 < s.len() && bytes[i + 1] == b'*' {
            if let Some(rel) = s[i + 2..].find("**") {
                let end = i + 2 + rel;
                flush_plain(&mut plain, &mut out);
                extend_emphasis(&mut out, &s[i + 2..end], SpanRole::Bold);
                i = end + 2;
                continue;
            }
            // Unmatched `**` — drop both stars so they don't paint.
            i += 2;
            continue;
        }
        // Bold: __...__
        if bytes[i] == b'_' && i + 1 < s.len() && bytes[i + 1] == b'_' {
            if let Some(rel) = s[i + 2..].find("__") {
                let end = i + 2 + rel;
                flush_plain(&mut plain, &mut out);
                extend_emphasis(&mut out, &s[i + 2..end], SpanRole::Bold);
                i = end + 2;
                continue;
            }
            i += 2;
            continue;
        }
        // Italic: *...* (not **)
        if bytes[i] == b'*' && (i + 1 >= s.len() || bytes[i + 1] != b'*') {
            if let Some(end) = find_italic_close(s, i + 1, b'*') {
                flush_plain(&mut plain, &mut out);
                extend_emphasis(&mut out, &s[i + 1..end], SpanRole::Italic);
                i = end + 1;
                continue;
            }
            i += 1;
            continue;
        }
        // Italic: _..._ (not __). Skip mid-identifier `_` so snake_case like
        // `usd_exchange_rate` stays intact (only boundary-flanked _italic_).
        if bytes[i] == b'_' && (i + 1 >= s.len() || bytes[i + 1] != b'_') {
            let prev_word = i > 0
                && s[..i]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_ascii_alphanumeric());
            if !prev_word {
                if let Some(end) = find_italic_close(s, i + 1, b'_') {
                    let next_word = s[end + 1..]
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_alphanumeric());
                    if !next_word {
                        flush_plain(&mut plain, &mut out);
                        extend_emphasis(&mut out, &s[i + 1..end], SpanRole::Italic);
                        i = end + 1;
                        continue;
                    }
                }
            }
            // Mid-word or unmatched: keep the underscore as plain text.
            plain.push('_');
            i += 1;
            continue;
        }
        let ch = s[i..].chars().next().unwrap();
        plain.push(ch);
        i += ch.len_utf8();
    }
    flush_plain(&mut plain, &mut out);
    if out.is_empty() {
        out.push((String::new(), SpanRole::Body));
    }
    out
}

/// Split plain text into Body / Link spans on http(s) URLs.
fn split_urls(s: String) -> Vec<(String, SpanRole)> {
    if s.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut rest = s.as_str();
    while !rest.is_empty() {
        let https = rest.find("https://");
        let http = rest.find("http://");
        let start = match (https, http) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        let Some(start) = start else {
            out.push((rest.to_string(), SpanRole::Body));
            break;
        };
        if start > 0 {
            out.push((rest[..start].to_string(), SpanRole::Body));
        }
        let url_rest = &rest[start..];
        let scheme_len = if url_rest.starts_with("https://") { 8 } else { 7 };
        let stop = url_rest
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | '>' | ']' | '"' | '\''))
            .unwrap_or(url_rest.len());
        if stop > scheme_len {
            out.push((url_rest[..stop].to_string(), SpanRole::Link));
            rest = &url_rest[stop..];
        } else {
            // scheme-only / no host — keep from the scheme onward as body.
            out.push((url_rest.to_string(), SpanRole::Body));
            break;
        }
    }
    out
}

/// Re-parse emphasis interior so nested `` `code` `` / markers still strip.
fn extend_emphasis(out: &mut Vec<(String, SpanRole)>, inner: &str, outer: SpanRole) {
    if inner.is_empty() {
        return;
    }
    for (text, role) in parse_inline(inner) {
        if text.is_empty() {
            continue;
        }
        let r = match role {
            SpanRole::Body => outer,
            other => other,
        };
        out.push((text, r));
    }
}

/// Find closing italic marker at `delim` that is not doubled (not `**` / `__`).
fn find_italic_close(s: &str, from: usize, delim: u8) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut j = from;
    while j < bytes.len() {
        if bytes[j] == delim {
            let doubled = j + 1 < bytes.len() && bytes[j + 1] == delim;
            if !doubled {
                if j > from {
                    return Some(j);
                }
            } else {
                j += 2;
                continue;
            }
        }
        if bytes[j] == b'`' {
            if let Some(end) = s[j + 1..].find('`').map(|o| j + 1 + o) {
                j = end + 1;
                continue;
            }
        }
        j += 1;
    }
    None
}

/// `^---+$` — three or more dashes, nothing else.
fn is_rule(line: &str) -> bool {
    line.len() >= 3 && line.bytes().all(|b| b == b'-')
}

#[cfg(test)]
mod tests {
    mod sanitize {
        use crate::markup::sanitize_display_line;

        #[test]
        fn strips_ansi_resolves_cr_and_expands_tabs() {
            // The real bytes from a vitest verify run (report.md line 30/31).
            assert_eq!(
                sanitize_display_line("\u{1b}[90mstderr\u{1b}[2m | api.test.ts\u{1b}[22m > ok"),
                "stderr | api.test.ts > ok"
            );
            // Spinner overwrites collapse to the final segment; CRLF tail drops.
            assert_eq!(sanitize_display_line("spin\rspun\rfinal"), "final");
            assert_eq!(sanitize_display_line("crlf line\r"), "crlf line");
            // OSC (hyperlink/title) sequences vanish with both terminators.
            assert_eq!(sanitize_display_line("\u{1b}]8;;http://x\u{1b}\\link\u{1b}]8;;\u{1b}\\"), "link");
            assert_eq!(sanitize_display_line("\u{1b}]0;title\u{7}text"), "text");
            // Tabs expand instead of silently collapsing to width 0.
            assert_eq!(sanitize_display_line("a\tb"), "a    b");
            // Plain text is untouched.
            assert_eq!(sanitize_display_line("plain — text"), "plain — text");
        }
    }

    use super::*;

    fn parts(line: &Line) -> Vec<(String, Style)> {
        line.spans
            .iter()
            .map(|s| (s.content.to_string(), s.style))
            .collect()
    }

    fn bold() -> Style {
        Style::default().fg(MD_EMPH).add_modifier(Modifier::BOLD)
    }
    fn plain() -> Style {
        Style::default()
    }
    fn rule(p: &Palette) -> Style {
        Style::default().fg(p.border)
    }
    fn code() -> Style {
        Style::default().fg(MD_CODE)
    }
    fn heading() -> Style {
        Style::default().fg(MD_HEADING).add_modifier(Modifier::BOLD)
    }
    fn link(p: &Palette) -> Style {
        Style::default().fg(p.accent)
    }
    fn ok(p: &Palette) -> Style {
        Style::default().fg(p.ok)
    }
    fn warn(p: &Palette) -> Style {
        Style::default().fg(p.warn)
    }
    fn accent(p: &Palette) -> Style {
        Style::default().fg(p.accent)
    }
    fn mauve(p: &Palette) -> Style {
        Style::default().fg(p.mauve)
    }
    fn joined(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn styles_headings_violet_and_strips_markers() {
        let p = Palette::default();
        // juice.ai discuss: strip `#` marks; paint body in violet+bold.
        assert_eq!(parts(&style_line("## Findings", &p)), vec![("Findings".into(), heading())]);
        assert_eq!(parts(&style_line("# Title", &p)), vec![("Title".into(), heading())]);
        assert_eq!(parts(&style_line("### Deep", &p)), vec![("Deep".into(), heading())]);
        assert_eq!(parts(&style_line("#### Four", &p)), vec![("Four".into(), heading())]);
        assert_eq!(parts(&style_line("###### Six", &p)), vec![("Six".into(), heading())]);
        // Nested code inside a heading stays mint.
        let got = parts(&style_line("### Tool: `Bash`", &p));
        assert_eq!(joined(&style_line("### Tool: `Bash`", &p)), "Tool: Bash");
        assert!(got.iter().any(|(t, s)| t == "Bash" && *s == code()));
    }

    #[test]
    fn seven_hashes_or_no_space_are_not_headings() {
        let p = Palette::default();
        assert_eq!(parts(&style_line("####### Seven", &p)), vec![("####### Seven".into(), plain())]);
        assert_eq!(parts(&style_line("#hash", &p)), vec![("#hash".into(), plain())]);
    }

    #[test]
    fn renders_a_horizontal_rule_of_three_or_more_dashes_in_border_color() {
        let p = Palette::default();
        assert_eq!(parts(&style_line("---", &p)), vec![("---".into(), rule(&p))]);
        assert_eq!(parts(&style_line("-----", &p)), vec![("-----".into(), rule(&p))]);
        assert_eq!(parts(&style_line("--", &p)), vec![("--".into(), plain())]);
    }

    #[test]
    fn plain_text_is_a_single_plain_segment() {
        let p = Palette::default();
        assert_eq!(
            parts(&style_line("just some text", &p)),
            vec![("just some text".into(), plain())]
        );
    }

    #[test]
    fn bolds_double_star_spans_gold_and_strips_markers() {
        let p = Palette::default();
        assert_eq!(
            parts(&style_line("see **Full report:** here", &p)),
            vec![
                ("see ".into(), plain()),
                ("Full report:".into(), bold()),
                (" here".into(), plain()),
            ]
        );
    }

    #[test]
    fn colors_inline_code_mint_and_strips_backticks() {
        let p = Palette::default();
        assert_eq!(
            parts(&style_line("call `foo.py:275` now", &p)),
            vec![
                ("call ".into(), plain()),
                ("foo.py:275".into(), code()),
                (" now".into(), plain()),
            ]
        );
    }

    #[test]
    fn colors_urls_blue() {
        let p = Palette::default();
        assert_eq!(
            parts(&style_line("link https://example.com/x done", &p)),
            vec![
                ("link ".into(), plain()),
                ("https://example.com/x".into(), link(&p)),
                (" done".into(), plain()),
            ]
        );
    }

    #[test]
    fn styles_multiple_spans_in_one_line() {
        let p = Palette::default();
        assert_eq!(
            parts(&style_line("**Full report:** `pr.md` at https://x.io", &p)),
            vec![
                ("Full report:".into(), bold()),
                (" ".into(), plain()),
                ("pr.md".into(), code()),
                (" at ".into(), plain()),
                ("https://x.io".into(), link(&p)),
            ]
        );
    }

    #[test]
    fn scheme_only_urls_stay_plain() {
        let p = Palette::default();
        for s in ["see http:// done", "https://", "http://)"] {
            let line = style_line(s, &p);
            assert_eq!(joined(&line), s);
            assert!(
                parts(&line).iter().all(|(_, st)| *st == plain()),
                "scheme-only must stay plain body, got {:?}",
                parts(&line)
            );
        }
    }

    #[test]
    fn unclosed_bold_drops_markers() {
        // juice.ai: unmatched `**` is dropped rather than painted.
        let p = Palette::default();
        assert_eq!(
            parts(&style_line("a **b never closes", &p)),
            vec![("a b never closes".into(), plain())]
        );
    }

    #[test]
    fn returns_one_segment_for_an_empty_line() {
        let p = Palette::default();
        assert_eq!(parts(&style_line("", &p)), vec![("".into(), plain())]);
    }

    #[test]
    fn italic_strips_markers_and_uses_gold() {
        let p = Palette::default();
        let italic = Style::default().fg(MD_EMPH).add_modifier(Modifier::ITALIC);
        assert_eq!(
            parts(&style_line("the *inverse* of", &p)),
            vec![
                ("the ".into(), plain()),
                ("inverse".into(), italic),
                (" of".into(), plain()),
            ]
        );
        // Underscore italic only at word boundaries — snake_case stays intact.
        assert_eq!(joined(&style_line("usd_exchange_rate", &p)), "usd_exchange_rate");
        assert_eq!(
            parts(&style_line("the _inverse_ of", &p)),
            vec![
                ("the ".into(), plain()),
                ("inverse".into(), italic),
                (" of".into(), plain()),
            ]
        );
    }

    #[test]
    fn nested_code_inside_bold() {
        let p = Palette::default();
        assert_eq!(joined(&style_line("**`col` vs `other`**", &p)), "col vs other");
        let got = parts(&style_line("**`col` vs `other`**", &p));
        assert!(got.iter().any(|(t, s)| t == "col" && *s == code()));
        assert!(got.iter().any(|(t, s)| t == "other" && *s == code()));
    }

    // ---- jinja overlay (applied by style_transcript_line) ------------------

    /// Style a plain-text line the way the renderer does (Text ctx), so the
    /// `{{jinja}}` overlay runs.
    fn text(line: &str, p: &Palette) -> Vec<(String, Style)> {
        parts(&style_transcript_line(line, &LineCtx::Text, 80, p))
    }

    #[test]
    fn jinja_placeholder_is_warn_in_prose() {
        let p = Palette::default();
        assert_eq!(
            text("hello {{name}} bye", &p),
            vec![
                ("hello ".into(), plain()),
                ("{{name}}".into(), warn(&p)),
                (" bye".into(), plain()),
            ]
        );
    }

    #[test]
    fn jinja_inside_inline_code_overrides_mint_to_warn() {
        let p = Palette::default();
        // The code span is mint; the placeholder within it is re-colored yellow.
        assert_eq!(
            text("run `{{cmd}} x`", &p),
            vec![
                ("run ".into(), plain()),
                ("{{cmd}}".into(), warn(&p)),
                (" x".into(), code()),
            ]
        );
    }

    #[test]
    fn jinja_inside_fenced_block_is_warn() {
        let p = Palette::default();
        // Unknown fenced language → plain body; the placeholder still styles.
        let got = parts(&style_transcript_line(
            "value = {{var}}",
            &LineCtx::Fenced { lang: "rust".into() },
            80,
            &p,
        ));
        assert_eq!(
            got,
            vec![
                ("value = ".into(), plain()),
                ("{{var}}".into(), warn(&p)),
            ]
        );
    }

    #[test]
    fn lone_open_braces_without_close_stay_unstyled() {
        let p = Palette::default();
        assert_eq!(text("a {{ b never closes", &p), vec![("a {{ b never closes".into(), plain())]);
    }

    #[test]
    fn jinja_non_greedy_closes_at_nearest_braces() {
        let p = Palette::default();
        // First `}}` closes the placeholder; the trailing text stays plain.
        assert_eq!(
            text("{{a}} and {{b}}", &p),
            vec![
                ("{{a}}".into(), warn(&p)),
                (" and ".into(), plain()),
                ("{{b}}".into(), warn(&p)),
            ]
        );
    }

    // ---- list markers ------------------------------------------------------

    #[test]
    fn bullet_marker_is_dim_bullet_glyph() {
        let p = Palette::default();
        // juice.ai: strip `- `/`* ` and paint a dim `• `.
        assert_eq!(
            parts(&style_line("- item one", &p)),
            vec![("• ".into(), dim(&p)), ("item one".into(), plain())]
        );
        assert_eq!(
            parts(&style_line("  * nested", &p)),
            vec![
                ("  ".into(), plain()),
                ("• ".into(), dim(&p)),
                ("nested".into(), plain()),
            ]
        );
    }

    #[test]
    fn ordered_marker_becomes_dim_bullet() {
        let p = Palette::default();
        assert_eq!(
            parts(&style_line("1. first", &p)),
            vec![("• ".into(), dim(&p)), ("first".into(), plain())]
        );
        assert_eq!(
            parts(&style_line("2) second", &p)),
            vec![("• ".into(), dim(&p)), ("second".into(), plain())]
        );
    }

    #[test]
    fn leading_double_star_is_bold_not_a_bullet() {
        let p = Palette::default();
        // `**` must not read as a `*` bullet — the bold tokenizer owns it.
        assert_eq!(parts(&style_line("**hi**", &p)), vec![("hi".into(), bold())]);
    }

    #[test]
    fn table_block_uses_grok_full_grid_borders() {
        let p = Palette::default();
        // Full GFM table → Grok grid: top rule, header, mid, data, bottom.
        let lines = vec![
            "| Bucket | Count | Item |".into(),
            "|---|---|---|".into(),
            "| FIX | 1 | long item text that wraps within the column |".into(),
            "| DROPPED | 0 | - |".into(),
        ];
        let ctxs = vec![LineCtx::Text; lines.len()];
        let display = wrap_lines(&lines, &ctxs, 48);
        assert!(display.len() >= 5, "top+header+mid+data+bottom, got {}", display.len());
        assert!(display.iter().all(|d| d.md_roles.is_some()));

        // Outer frame characters.
        assert!(
            display[0].text.starts_with('┌') && display[0].text.ends_with('┐'),
            "top border, got {:?}",
            display[0].text
        );
        assert!(
            display.last().unwrap().text.starts_with('└')
                && display.last().unwrap().text.ends_with('┘'),
            "bottom border, got {:?}",
            display.last().unwrap().text
        );
        assert!(
            display.iter().any(|d| d.text.starts_with('├') && d.text.contains('┼')),
            "mid row rule with ┼ expected"
        );

        // Header content row: bold (Grok), vertical bars dim.
        let header = style_display_line(&display[1], 48, &p);
        assert!(joined(&header).contains("Bucket"));
        assert!(
            parts(&header)
                .iter()
                .any(|(t, s)| t.contains("Bucket") && *s == bold()),
            "header cells bold, got {:?}",
            parts(&header)
        );
        assert!(
            parts(&header)
                .iter()
                .any(|(t, s)| t == "│" && *s == dim(&p)),
            "vertical borders dim"
        );

        // Data: DROPPED is body, not bold header.
        let dropped = display
            .iter()
            .find(|d| d.text.contains("DROPPED"))
            .expect("DROPPED row");
        let painted = style_display_line(dropped, 48, &p);
        assert!(
            !parts(&painted)
                .iter()
                .any(|(t, s)| t.contains("DROPPED") && *s == bold()),
            "data cell must not be header-bold, got {:?}",
            parts(&painted)
        );
    }

    #[test]
    fn table_long_cell_wraps_inside_column() {
        let lines = vec![
            "| A | B |".into(),
            "|---|---|".into(),
            "| x | one two three four five six seven eight |".into(),
        ];
        let ctxs = vec![LineCtx::Text; lines.len()];
        let display = wrap_lines(&lines, &ctxs, 28);
        // Content rows that carry cell text (skip pure border rules).
        let content: Vec<_> = display
            .iter()
            .filter(|d| d.text.starts_with('│'))
            .collect();
        assert!(
            content.len() >= 3,
            "header + multi-line data body expected, got {}",
            content.len()
        );
        // Wrapped data continuations still open with a vertical bar (grid intact).
        for row in &content {
            assert!(row.text.starts_with('│'), "grid row must start with │: {:?}", row.text);
        }
    }

    // ---- fence_states ------------------------------------------------------

    fn kinds(lines: &[&str]) -> Vec<LineCtx> {
        let owned: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
        fence_states(&owned)
    }

    #[test]
    fn fence_states_tracks_open_language_and_close() {
        let got = kinds(&["intro", "```bash", "echo hi", "```", "outro"]);
        assert_eq!(
            got,
            vec![
                LineCtx::Text,
                LineCtx::Fence { lang: Some("bash".into()) },
                LineCtx::Fenced { lang: "bash".into() },
                LineCtx::Fence { lang: None },
                LineCtx::Text,
            ]
        );
    }

    #[test]
    fn fence_states_bare_open_has_no_language() {
        let got = kinds(&["```", "plain body", "```"]);
        assert_eq!(
            got,
            vec![
                LineCtx::Fence { lang: None },
                LineCtx::Fenced { lang: String::new() },
                LineCtx::Fence { lang: None },
            ]
        );
    }

    #[test]
    fn fence_states_leaves_unclosed_block_fenced_to_eof() {
        let got = kinds(&["```json", "{\"a\": 1}", "still inside"]);
        assert_eq!(
            got,
            vec![
                LineCtx::Fence { lang: Some("json".into()) },
                LineCtx::Fenced { lang: "json".into() },
                LineCtx::Fenced { lang: "json".into() },
            ]
        );
    }

    #[test]
    fn fence_states_does_not_nest_second_fence_closes_first() {
        // A second ``` inside the block closes it; a following ```py opens anew.
        let got = kinds(&["```sh", "a", "```", "```py", "b", "```"]);
        assert_eq!(
            got,
            vec![
                LineCtx::Fence { lang: Some("sh".into()) },
                LineCtx::Fenced { lang: "sh".into() },
                LineCtx::Fence { lang: None },
                LineCtx::Fence { lang: Some("py".into()) },
                LineCtx::Fenced { lang: "py".into() },
                LineCtx::Fence { lang: None },
            ]
        );
    }

    fn kinds_from(lines: &[&str], starts_in_fence: bool) -> Vec<LineCtx> {
        let owned: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
        fence_states_from(&owned, starts_in_fence)
    }

    #[test]
    fn fence_states_from_false_matches_fence_states() {
        // The `starts_in_fence == false` arm is exactly what every existing
        // caller gets through the `fence_states` delegate.
        let lines = &["intro", "```bash", "echo hi", "```", "outro"];
        assert_eq!(kinds_from(lines, false), kinds(lines));
    }

    #[test]
    fn fence_states_from_true_starts_inside_a_fence() {
        // A tail window that opened mid-fence: the first lines are fenced code,
        // the first bare ``` CLOSES the fence, and the prose after is Text.
        let got = kinds_from(&["make build", "make test", "```", "### Tool: Bash"], true);
        assert_eq!(
            got,
            vec![
                LineCtx::Fenced { lang: String::new() },
                LineCtx::Fenced { lang: String::new() },
                LineCtx::Fence { lang: None },
                LineCtx::Text,
            ]
        );
    }

    #[test]
    fn unlabeled_fence_with_markdown_body_is_reclassed() {
        // Agent dump: bare ``` then prose with headings/bold — paint as markdown.
        let lines = [
            "intro",
            "```",
            "Found the failure:",
            "",
            "**Summary:** root cause is X.",
            "",
            "## Details",
            "- item one",
            "- item two",
            "more prose about the fix",
            "```",
            "outro",
        ];
        let got = kinds(&lines);
        assert_eq!(got[0], LineCtx::Text);
        assert_eq!(got[1], LineCtx::Fence { lang: None });
        // Body re-tagged markdown.
        for ctx in &got[2..10] {
            assert_eq!(
                ctx,
                &LineCtx::Fenced {
                    lang: "markdown".into()
                },
                "expected markdown reclass, got {ctx:?}"
            );
        }
        assert_eq!(got[10], LineCtx::Fence { lang: None });
        assert_eq!(got[11], LineCtx::Text);

        let p = Palette::default();
        let summary = style_transcript_line(
            "**Summary:** root cause is X.",
            &LineCtx::Fenced {
                lang: "markdown".into(),
            },
            80,
            &p,
        );
        assert!(
            parts(&summary).iter().any(|(_, s)| *s == bold()),
            "markdown fence must bold **…**, got {:?}",
            parts(&summary)
        );
        let h = style_transcript_line(
            "## Details",
            &LineCtx::Fenced {
                lang: "markdown".into(),
            },
            80,
            &p,
        );
        assert_eq!(joined(&h), "Details");
        assert!(parts(&h).iter().any(|(_, s)| *s == heading()));
    }

    #[test]
    fn unlabeled_short_code_fence_is_not_reclassed_as_markdown() {
        let lines = ["```", "make build", "make test", "```"];
        let got = kinds(&lines);
        assert_eq!(
            got,
            vec![
                LineCtx::Fence { lang: None },
                LineCtx::Fenced { lang: String::new() },
                LineCtx::Fenced { lang: String::new() },
                LineCtx::Fence { lang: None },
            ]
        );
    }

    #[test]
    fn explicit_markdown_fence_uses_prose_styler() {
        let p = Palette::default();
        let got = parts(&style_transcript_line(
            "call `foo` and **bar**",
            &LineCtx::Fenced {
                lang: "markdown".into(),
            },
            80,
            &p,
        ));
        assert!(got.iter().any(|(t, s)| t == "foo" && *s == code()));
        assert!(got.iter().any(|(t, s)| t == "bar" && *s == bold()));
    }

    #[test]
    fn generic_code_fence_accents_strings_and_comments() {
        let p = Palette::default();
        let s = parts(&style_transcript_line(
            r#"x = "hello"  # note"#,
            &LineCtx::Fenced { lang: "python".into() },
            80,
            &p,
        ));
        assert!(
            s.iter().any(|(t, st)| t.contains("hello") && *st == code()),
            "string mint, got {s:?}"
        );
        assert!(
            s.iter().any(|(t, st)| t.contains("# note") && *st == dim(&p)),
            "comment dim, got {s:?}"
        );
    }

    // ---- windowed slice ----------------------------------------------------

    #[test]
    fn windowed_slice_mid_block_styles_as_code() {
        // Precompute over the whole vec, then style only a middle window. The
        // sliced line must still know it is fenced bash and accent accordingly.
        let lines: Vec<String> = vec![
            "before".into(),
            "```bash".into(),
            "make build".into(),
            "make test".into(),
            "```".into(),
        ];
        let ctxs = fence_states(&lines);
        let p = Palette::default();
        // Window == [2..4], exactly the two body lines (fence delimiters clipped).
        let styled: Vec<Line> = lines[2..4]
            .iter()
            .enumerate()
            .map(|(off, l)| style_transcript_line(l, &ctxs[2 + off], 40, &p))
            .collect();
        // First token of each body line is a command → green.
        assert_eq!(styled[0].spans[0].content, "make");
        assert_eq!(styled[0].spans[0].style, ok(&p));
        assert_eq!(styled[1].spans[0].content, "make");
        assert_eq!(styled[1].spans[0].style, ok(&p));
    }

    // ---- rule rendering ----------------------------------------------------

    #[test]
    fn opening_fence_renders_labeled_rule() {
        let p = Palette::default();
        let line = style_transcript_line("```bash", &LineCtx::Fence { lang: Some("bash".into()) }, 30, &p);
        let got = parts(&line);
        assert_eq!(got.len(), 3);
        assert_eq!(got[0], (RULE_CHAR.to_string().repeat(FENCE_RULE_PREFIX), rule(&p)));
        assert_eq!(got[1], (" bash ".to_string(), p.dim_style()));
        // prefix(8) + " bash "(6) + trailing = 30 → trailing 16.
        assert_eq!(got[2], (RULE_CHAR.to_string().repeat(16), rule(&p)));
    }

    #[test]
    fn closing_fence_renders_plain_full_width_rule() {
        let p = Palette::default();
        let line = style_transcript_line("```", &LineCtx::Fence { lang: None }, 12, &p);
        assert_eq!(parts(&line), vec![(RULE_CHAR.to_string().repeat(12), rule(&p))]);
    }

    #[test]
    fn labeled_rule_keeps_minimum_trailing_on_narrow_pane() {
        let p = Palette::default();
        let line = style_transcript_line("```bash", &LineCtx::Fence { lang: Some("bash".into()) }, 4, &p);
        let got = parts(&line);
        assert_eq!(got[2].0.chars().count(), FENCE_RULE_MIN_TRAIL);
    }

    // ---- bash accents ------------------------------------------------------

    fn bash(line: &str, p: &Palette) -> Vec<(String, Style)> {
        parts(&style_transcript_line(line, &LineCtx::Fenced { lang: "bash".into() }, 80, p))
    }

    #[test]
    fn bash_first_token_and_post_pipeline_token_are_commands() {
        let p = Palette::default();
        assert_eq!(
            bash("cat file.txt | grep foo", &p),
            vec![
                ("cat".into(), ok(&p)),
                (" ".into(), plain()),
                ("file.txt".into(), plain()),
                (" ".into(), plain()),
                ("|".into(), plain()),
                (" ".into(), plain()),
                ("grep".into(), ok(&p)),
                (" ".into(), plain()),
                ("foo".into(), plain()),
            ]
        );
    }

    #[test]
    fn bash_command_after_logical_and_is_a_command() {
        let p = Palette::default();
        assert_eq!(
            bash("ls /usr && cd ~/proj", &p),
            vec![
                ("ls".into(), ok(&p)),
                (" ".into(), plain()),
                ("/usr".into(), accent(&p)),
                (" ".into(), plain()),
                ("&&".into(), plain()),
                (" ".into(), plain()),
                ("cd".into(), ok(&p)),
                (" ".into(), plain()),
                ("~/proj".into(), accent(&p)),
            ]
        );
    }

    #[test]
    fn bash_quotes_are_yellow_and_paths_blue() {
        let p = Palette::default();
        assert_eq!(
            bash("echo \"hello world\" ./run.sh", &p),
            vec![
                ("echo".into(), ok(&p)),
                (" ".into(), plain()),
                ("\"hello world\"".into(), warn(&p)),
                (" ".into(), plain()),
                ("./run.sh".into(), accent(&p)),
            ]
        );
    }

    #[test]
    fn bash_command_position_wins_over_path_prefix() {
        let p = Palette::default();
        // Leading ./script is a command → green, not blue.
        let got = bash("./deploy.sh --prod", &p);
        assert_eq!(got[0], ("./deploy.sh".into(), ok(&p)));
    }

    // ---- json accents ------------------------------------------------------

    fn json(line: &str, p: &Palette) -> Vec<(String, Style)> {
        parts(&style_transcript_line(line, &LineCtx::Fenced { lang: "json".into() }, 80, p))
    }

    #[test]
    fn json_keys_strings_numbers_and_literals() {
        let p = Palette::default();
        assert_eq!(
            json("\"name\": \"qoo\"", &p),
            vec![
                ("\"name\"".into(), accent(&p)),
                (": ".into(), plain()),
                ("\"qoo\"".into(), ok(&p)),
            ]
        );
        assert_eq!(
            json("\"count\": 42", &p),
            vec![
                ("\"count\"".into(), accent(&p)),
                (": ".into(), plain()),
                ("42".into(), mauve(&p)),
            ]
        );
        assert_eq!(
            json("\"ok\": true", &p),
            vec![
                ("\"ok\"".into(), accent(&p)),
                (": ".into(), plain()),
                ("true".into(), mauve(&p)),
            ]
        );
    }

    #[test]
    fn json_literal_not_matched_inside_a_word() {
        let p = Palette::default();
        // "nullable" (unquoted) must not read as null + able.
        let got = json("nullable", &p);
        assert_eq!(got, vec![("nullable".into(), plain())]);
    }

    #[test]
    fn json_multibyte_chars_do_not_panic_and_reconstruct() {
        let p = Palette::default();
        // Regression: the plain-segment scan stepped byte-wise, so an unquoted
        // multi-byte char (here `–`) put the cursor mid-char and the
        // `json_literal_at(&line[i..])` slice panicked on a non-boundary index.
        for line in ["a– b", "Q1–Q3: 42", "✓ done – \"ok\": true", "–"] {
            let got = json(line, &p);
            let joined: String = got.iter().map(|(t, _)| t.as_str()).collect();
            assert_eq!(joined, line);
        }
    }

    // ---- wrap_lines --------------------------------------------------------

    /// Wrap `lines` (fence ctxs derived like the renderer) and flatten to
    /// `(text, is_continuation)` pairs for terse assertions.
    fn wrapped(lines: &[&str], width: usize) -> Vec<(String, bool)> {
        let owned: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
        let ctxs = fence_states(&owned);
        wrap_lines(&owned, &ctxs, width)
            .into_iter()
            .map(|d| (d.text, d.is_continuation))
            .collect()
    }

    #[test]
    fn wrap_word_wraps_prose_at_spaces() {
        assert_eq!(
            wrapped(&["the quick brown fox"], 9),
            vec![("the quick".into(), false), ("brown fox".into(), true)]
        );
    }

    #[test]
    fn wrap_exact_width_line_does_not_wrap() {
        // Exactly `width` cells → one segment, byte-for-byte.
        assert_eq!(wrapped(&["abcdefghij"], 10), vec![("abcdefghij".into(), false)]);
        // One over → wraps.
        assert_eq!(
            wrapped(&["abcdefghijk"], 10),
            vec![("abcdefghij".into(), false), ("k".into(), true)]
        );
    }

    #[test]
    fn wrap_hard_breaks_an_over_wide_token() {
        // A URL longer than the width has no space to break at → hard-break at the
        // cell boundary. "https://example.com/" is exactly 20 cells.
        assert_eq!(
            wrapped(&["https://example.com/abcdefghij"], 20),
            vec![("https://example.com/".into(), false), ("abcdefghij".into(), true)]
        );
    }

    #[test]
    fn wrap_prose_then_hard_breaks_long_url() {
        let got = wrapped(&["go https://example.com/abcdefghij now"], 20);
        assert_eq!(got[0], ("go".into(), false));
        assert_eq!(got[1], ("https://example.com/".into(), true));
        // Every segment fits the width in CELLS.
        for (text, _) in &got {
            assert!(str_width(text) <= 20, "segment {text:?} overflows width");
        }
    }

    #[test]
    fn wrap_is_cell_width_aware_for_multiwidth_chars() {
        // Five CJK chars (2 cells each = 10 cells) into width 6 → 3+2 chars, never
        // 6+... a char-count wrapper would have kept all five on one 12-cell row.
        assert_eq!(
            wrapped(&["中中中中中"], 6),
            vec![("中中中".into(), false), ("中中".into(), true)]
        );
    }

    #[test]
    fn wrap_keeps_empty_line_as_one_empty_display_line() {
        assert_eq!(
            wrapped(&["", "x"], 10),
            vec![("".into(), false), ("x".into(), false)]
        );
    }

    #[test]
    fn wrap_preserves_first_line_indent_continuations_flush_left() {
        assert_eq!(
            wrapped(&["    indented text that is quite long here"], 12),
            vec![
                ("    indented".into(), false),
                ("text that is".into(), true),
                ("quite long".into(), true),
                ("here".into(), true),
            ]
        );
    }

    #[test]
    fn wrap_passes_fence_rule_lines_through_unwrapped() {
        // An opening fence whose raw text far exceeds the width stays ONE segment
        // (the renderer regenerates it as a sized rule); it must not be wrapped.
        let owned: Vec<String> =
            ["```averylonglanguagenamethatexceeds", "code", "```"].map(String::from).into();
        let ctxs = fence_states(&owned);
        let got = wrap_lines(&owned, &ctxs, 10);
        assert_eq!(got[0].text, "```averylonglanguagenamethatexceeds");
        assert!(!got[0].is_continuation);
        assert!(matches!(got[0].ctx, LineCtx::Fence { .. }));
    }

    #[test]
    fn wrap_fenced_continuations_keep_lang_ctx() {
        // A long bash line hard-breaks; every continuation keeps Fenced{bash} so
        // syntax accents carry across the wrap.
        let owned: Vec<String> =
            ["```bash", "echo aaaaaaaaaaaaaaaaaaaaaaaaaaaa", "```"].map(String::from).into();
        let ctxs = fence_states(&owned);
        let body: Vec<DisplayLine> = wrap_lines(&owned, &ctxs, 12)
            .into_iter()
            .filter(|d| matches!(d.ctx, LineCtx::Fenced { .. }))
            .collect();
        assert!(body.len() > 1, "long fenced line wrapped into multiple segments");
        assert!(body.iter().all(|d| d.ctx == LineCtx::Fenced { lang: "bash".into() }));
    }

    // ---- cell ↔ char mapping (detail selection) ---------------------------

    #[test]
    fn char_at_cell_maps_ascii_columns_and_clamps_past_end() {
        assert_eq!(char_at_cell("hello", 0), 0);
        assert_eq!(char_at_cell("hello", 4), 4);
        // Past the end clamps to the last char (click in trailing padding).
        assert_eq!(char_at_cell("hello", 99), 4);
        // Empty text has no char → 0.
        assert_eq!(char_at_cell("", 3), 0);
    }

    #[test]
    fn char_at_cell_handles_double_width_chars() {
        // "中" is 2 cells; "中x" occupies cells [0,1]=中, [2]=x.
        assert_eq!(char_at_cell("中x", 0), 0); // first cell of 中
        assert_eq!(char_at_cell("中x", 1), 0); // second cell of 中 → same char
        assert_eq!(char_at_cell("中x", 2), 1); // x
    }

    #[test]
    fn slice_cells_extracts_inclusive_ascii_range() {
        assert_eq!(slice_cells("hello world", 0, 4), "hello");
        assert_eq!(slice_cells("hello world", 6, 10), "world");
        // MAX sentinel selects through end-of-line.
        assert_eq!(slice_cells("hello world", 6, usize::MAX), "world");
        // Whole line from column 0.
        assert_eq!(slice_cells("abc", 0, usize::MAX), "abc");
        assert_eq!(slice_cells("", 0, usize::MAX), "");
    }

    #[test]
    fn slice_cells_is_multiwidth_aware_and_underflow_safe() {
        // Cells: 中(0,1) 中(2,3) x(4). Range [2,4] = second 中 + x.
        assert_eq!(slice_cells("中中x", 2, 4), "中x");
        // lo > hi (can arise after clamping) yields a safe single-char slice, not
        // a panic.
        let _ = slice_cells("abc", 5, 1);
    }

    // ---- config rows -------------------------------------------------------

    fn config(line: &str, key_col: usize, p: &Palette) -> Vec<(String, Style)> {
        parts(&style_transcript_line(line, &LineCtx::Config { key_col }, 80, p))
    }

    #[test]
    fn config_row_key_accent_value_default_grey() {
        // A generic value renders in the terminal-default grey (no fg override);
        // white is reserved for actions/tabs.
        let p = Palette::default();
        assert_eq!(
            config("dedup      none", 11, &p),
            vec![("dedup      ".into(), accent(&p)), ("none".into(), Style::default())]
        );
    }

    #[test]
    fn config_timestamp_value_is_teal_and_pr_is_meta_underlined() {
        // Same concept, same color as the panes: timestamps teal, pr the metadata
        // gold (underlined link affordance).
        let p = Palette::default();
        assert_eq!(
            config("updated    9h ago", 11, &p),
            vec![("updated    ".into(), accent(&p)), ("9h ago".into(), Style::default().fg(p.info))]
        );
        assert_eq!(
            config("pr         #1870", 11, &p),
            vec![
                ("pr         ".into(), accent(&p)),
                ("#1870".into(), Style::default().fg(p.meta).add_modifier(Modifier::UNDERLINED)),
            ]
        );
    }

    #[test]
    fn config_row_dims_em_dash_placeholder() {
        let p = Palette::default();
        assert_eq!(
            config("discovery  —", 11, &p),
            vec![("discovery  ".into(), accent(&p)), ("—".into(), p.dim_style())]
        );
    }

    #[test]
    fn config_model_value_is_meta_gold_with_dim_arrow() {
        // `model` reads in the metadata gold (matches the TASKS model column);
        // every chain entry is equal meta gold; arrows stay dim (no bold head).
        let p = Palette::default();
        let meta = Style::default().fg(p.meta);
        assert_eq!(
            config("model      grok-4.5 → claude-opus-4.8", 11, &p),
            vec![
                ("model      ".into(), accent(&p)),
                ("grok-4.5".into(), meta),
                (" → ".into(), p.dim_style()),
                ("claude-opus-4.8".into(), meta),
            ]
        );
        // Three-entry chain: every label equal, every arrow dim.
        assert_eq!(
            config("model      a → b → c", 11, &p),
            vec![
                ("model      ".into(), accent(&p)),
                ("a".into(), meta),
                (" → ".into(), p.dim_style()),
                ("b".into(), meta),
                (" → ".into(), p.dim_style()),
                ("c".into(), meta),
            ]
        );
    }

    #[test]
    fn config_continuation_key_col_zero_is_all_value() {
        // `key_col == 0` marks a wrapped continuation (no key column): the whole
        // segment styles as value, never re-coloring its start in the key accent.
        // Regression for the worktree info `path` row rendering `/Users…` blue.
        let p = Palette::default();
        assert_eq!(
            config("/Users/noootown/Downloads", 0, &p),
            vec![("/Users/noootown/Downloads".into(), Style::default())]
        );
    }

    #[test]
    fn wrapped_config_value_keys_only_the_first_segment() {
        // End-to-end: a `path   <long value>` row wrapped narrow keeps the accent
        // key column on the FIRST segment only; every continuation is pure value
        // (no accent-colored prefix). Reproduces the worktree `path` bug — a short
        // value (branch) never wraps, so only long values (paths) were affected.
        let p = Palette::default();
        let lines = vec!["path     /Users/noootown/Downloads/agent247/queohoh".to_string()];
        let ctxs = vec![LineCtx::Config { key_col: 9 }];
        let display = wrap_lines(&lines, &ctxs, 20);
        assert!(display.len() > 1, "the long path value wraps into continuations");
        let first = parts(&style_transcript_line(&display[0].text, &display[0].ctx, 20, &p));
        assert_eq!(first[0].1, accent(&p), "first segment keeps the accent key column");
        for seg in &display[1..] {
            let styled = parts(&style_transcript_line(&seg.text, &seg.ctx, 20, &p));
            assert!(
                styled.iter().all(|(_, st)| *st != accent(&p)),
                "continuation {:?} must not re-color any span as a key",
                seg.text
            );
        }
    }

    #[test]
    fn rust_fence_without_strings_is_plain_body() {
        // Generic code accent only colors strings/comments — bare tokens stay plain.
        let p = Palette::default();
        let got = parts(&style_transcript_line(
            "fn main() {}",
            &LineCtx::Fenced { lang: "rust".into() },
            80,
            &p,
        ));
        assert_eq!(got, vec![("fn main() {}".into(), plain())]);
    }

    // ---- lane-task rows + header (worktree detail list) --------------------

    /// Concatenated text of every span, so column alignment can be asserted by
    /// substring/offset without depending on exact span boundaries.
    fn flat(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn info(p: &Palette) -> Style {
        Style::default().fg(p.info)
    }
    fn dim(p: &Palette) -> Style {
        p.dim_style()
    }

    #[test]
    fn lane_task_row_lays_out_name_created_age_and_live_columns() {
        let p = Palette::default();
        let line = style_transcript_line(
            "squash-merge",
            &LineCtx::LaneTask {
                glyph: '▶',
                is_def: true,
                created: "07/09 07:00".into(),
                age: "just now".into(),
                live: "⏱ 3m12s".into(),
                selected: false,
            },
            60,
            &p,
        );
        let got = flat(&line);
        // Glyph leads; def name is mauve; created/age read teal; live reads warn.
        assert!(got.starts_with("▶ squash-merge"), "glyph + name lead: {got:?}");
        assert!(got.contains("07/09 07:00"), "created column present");
        assert!(got.contains("just now"), "age column present");
        assert!(got.contains("⏱ 3m12s"), "live column present");
        let styled: Vec<(String, Style)> = parts(&line);
        assert!(styled.iter().any(|(t, s)| t.starts_with("squash-merge") && *s == mauve(&p)));
        assert!(styled.iter().any(|(t, s)| t.contains("07/09 07:00") && *s == info(&p)));
        assert!(styled.iter().any(|(t, s)| t.contains("just now") && *s == info(&p)));
        assert!(styled.iter().any(|(t, s)| t.contains("⏱ 3m12s") && *s == warn(&p)));
    }

    #[test]
    fn lane_task_row_blank_live_is_plain_not_warn() {
        let p = Palette::default();
        // A finished task has an empty live cell — it must render as raw padding
        // (plain), never a warn-colored blank run.
        let line = style_transcript_line(
            "flaky migration",
            &LineCtx::LaneTask {
                glyph: '✗',
                is_def: false,
                created: "07/09 07:00".into(),
                age: "3d ago".into(),
                live: String::new(),
                selected: false,
            },
            60,
            &p,
        );
        let styled = parts(&line);
        assert!(
            !styled.iter().any(|(_, s)| *s == warn(&p)),
            "no warn span when the live cell is empty"
        );
        // Non-def (prompt) name renders in the terminal-default grey, not mauve.
        assert!(styled.iter().any(|(t, s)| t.starts_with("flaky migration") && *s == Style::default()));
    }

    #[test]
    fn lane_task_row_selected_inverts_every_span() {
        let p = Palette::default();
        let sel = p.selection();
        let line = style_transcript_line(
            "squash-merge",
            &LineCtx::LaneTask {
                glyph: '○',
                is_def: true,
                created: "07/09 07:04".into(),
                age: "just now".into(),
                live: "#1 in lane".into(),
                selected: true,
            },
            60,
            &p,
        );
        // Every span carries the selection patch (the whole row inverts).
        for span in &line.spans {
            assert_eq!(span.style, span.style.patch(sel), "span {:?} not selected", span.content);
        }
    }

    #[test]
    fn lane_header_row_labels_columns_in_dim_over_the_glyph_slot() {
        let p = Palette::default();
        let line = style_transcript_line("Task", &LineCtx::LaneHeader, 60, &p);
        let got = flat(&line);
        // No label over the 2-cell glyph slot; the four labels sit over columns.
        assert!(got.starts_with("  Task"), "glyph slot is blank, Task leads: {got:?}");
        for label in ["Task", "Created", "Age", "Live"] {
            assert!(got.contains(label), "{label} header present");
        }
        // Header labels are chrome → dim; nothing renders selected.
        let styled = parts(&line);
        assert!(styled.iter().any(|(t, s)| t.contains("Created") && *s == dim(&p)));
        assert!(styled.iter().any(|(t, s)| t.contains("Live") && *s == dim(&p)));
    }

    #[test]
    fn lane_header_aligns_cell_for_cell_with_a_row() {
        // The header's column starts must equal the row's — same `lane_task_cols`
        // width drives both. Assert the `Created`/`Age`/`Live` labels begin at the
        // exact cell offsets the row's values begin at.
        let p = Palette::default();
        let width = 60;
        let header = flat(&style_transcript_line("Task", &LineCtx::LaneHeader, width, &p));
        let row = flat(&style_transcript_line(
            "squash-merge",
            &LineCtx::LaneTask {
                glyph: '▶',
                is_def: true,
                created: "07/09 07:00".into(),
                age: "just now".into(),
                live: "#1 in lane".into(),
                selected: false,
            },
            width,
            &p,
        ));
        let cols = crate::selectors::lane_task_cols(width as usize);
        // Column start offsets (in chars, all ASCII here): glyph slot(2) + name +
        // gap, then +created+gap, then +age+gap.
        let created_at = 2 + cols.name_w + crate::selectors::COL_GAP;
        let age_at = created_at + cols.created_w + crate::selectors::COL_GAP;
        let live_at = age_at + cols.age_w + crate::selectors::COL_GAP;
        let at = |s: &str, n: usize| s.chars().skip(n).collect::<String>();
        assert!(at(&header, created_at).starts_with("Created"));
        assert!(at(&row, created_at).starts_with("07/09 07:00"));
        assert!(at(&header, age_at).starts_with("Age"));
        assert!(at(&row, age_at).starts_with("just now"));
        assert!(at(&header, live_at).starts_with("Live"));
        assert!(at(&row, live_at).starts_with("#1 in lane"));
    }
}
