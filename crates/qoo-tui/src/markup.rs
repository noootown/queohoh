use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::view::theme::{FENCE_RULE_MIN_TRAIL, FENCE_RULE_PREFIX, Palette, RULE_CHAR};

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
}

/// One cheap pass over the full transcript classifying each line. A line whose
/// trimmed content starts with ```` ``` ```` toggles the fence; the info string
/// after the backticks on an *opening* fence names the language. A second
/// ```` ``` ```` closes the block (so a nested fence just ends the first — there
/// is no nesting). An unclosed fence at EOF leaves trailing lines as `Fenced`.
pub fn fence_states(lines: &[String]) -> Vec<LineCtx> {
    let mut out = Vec::with_capacity(lines.len());
    // Some(lang) while inside a fence; None outside.
    let mut open: Option<String> = None;
    for line in lines {
        if let Some(rest) = line.trim_start().strip_prefix("```") {
            if open.is_none() {
                let info = rest.trim();
                let lang = if info.is_empty() { None } else { Some(info.to_string()) };
                open = Some(lang.clone().unwrap_or_default());
                out.push(LineCtx::Fence { lang });
            } else {
                open = None;
                out.push(LineCtx::Fence { lang: None });
            }
        } else if let Some(lang) = &open {
            out.push(LineCtx::Fenced { lang: lang.clone() });
        } else {
            out.push(LineCtx::Text);
        }
    }
    out
}

/// One display line produced by [`wrap_lines`]: a slice of an original logical
/// line, carrying the [`LineCtx`] it must be styled under. Continuation segments
/// (everything after the first) keep their line's ctx so fenced syntax accents
/// carry across the wrap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayLine {
    pub text: String,
    pub ctx: LineCtx,
    /// `false` for the first segment of a logical line, `true` for the rest.
    pub is_continuation: bool,
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
pub fn wrap_lines(lines: &[String], ctxs: &[LineCtx], width: usize) -> Vec<DisplayLine> {
    let width = width.max(1);
    let mut out = Vec::with_capacity(lines.len());
    for (line, ctx) in lines.iter().zip(ctxs.iter()) {
        // Fence delimiters and already-fitting/empty lines pass through as one
        // segment. `str_width("") == 0 <= width` folds the empty case in here.
        if matches!(ctx, LineCtx::Fence { .. }) || str_width(line) <= width {
            out.push(DisplayLine { text: line.clone(), ctx: ctx.clone(), is_continuation: false });
            continue;
        }
        let pieces = match ctx {
            LineCtx::Fenced { .. } => hard_break(line, width),
            _ => word_wrap(line, width),
        };
        for (i, text) in pieces.into_iter().enumerate() {
            out.push(DisplayLine { text, ctx: ctx.clone(), is_continuation: i > 0 });
        }
    }
    out
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
/// [`style_line`]. `width` is the content width the rules are sized to (any
/// overflow is clipped by the `Paragraph`).
pub fn style_transcript_line(line: &str, ctx: &LineCtx, width: u16, p: &Palette) -> Line<'static> {
    match ctx {
        LineCtx::Text => style_line(line, p),
        LineCtx::Fence { lang } => fence_rule(lang.as_deref(), width, p),
        LineCtx::Fenced { lang } => style_fenced(line, lang, p),
    }
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
    Bash,
    Json,
    Other,
}

impl FenceLang {
    fn classify(lang: &str) -> Self {
        match lang.trim().to_ascii_lowercase().as_str() {
            "bash" | "sh" | "shell" | "zsh" | "console" => FenceLang::Bash,
            "json" => FenceLang::Json,
            _ => FenceLang::Other,
        }
    }
}

/// Dispatch fenced content to a per-language accenter. Unknown languages render
/// as plain fg (rule-of-brightness: no flat wash).
fn style_fenced(line: &str, lang: &str, p: &Palette) -> Line<'static> {
    match FenceLang::classify(lang) {
        FenceLang::Bash => style_bash(line, p),
        FenceLang::Json => style_json(line, p),
        FenceLang::Other => Line::from(Span::raw(line.to_string())),
    }
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

/// Style one detail-pane text line (port of markup.ts styleLine). Whole-line
/// rules (headings, horizontal rules) win; otherwise the line is tokenized into
/// **bold** / `code` / URL spans with surrounding text plain. Returns an owned
/// Line — always at least one span.
pub fn style_line(line: &str, p: &Palette) -> Line<'static> {
    if let Some(text) = heading_text(line) {
        return Line::from(Span::styled(
            text.to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        ));
    }
    if is_rule(line) {
        // Border color, not the DIM modifier — grey-on-dark was unreadable, and
        // it now matches the fenced-block rules.
        return Line::from(Span::styled(line.to_string(), Style::default().fg(p.border)));
    }

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut last = 0usize;
    let mut i = 0usize;
    while i < line.len() {
        if !line.is_char_boundary(i) {
            i += 1;
            continue;
        }
        if let Some((end, span)) = match_token(line, i, p) {
            if i > last {
                spans.push(Span::raw(line[last..i].to_string()));
            }
            spans.push(span);
            last = end;
            i = end;
        } else {
            i += 1;
        }
    }
    if last < line.len() {
        spans.push(Span::raw(line[last..].to_string()));
    }
    if spans.is_empty() {
        spans.push(Span::raw(line.to_string()));
    }
    Line::from(spans)
}

/// `^#{1,3}\s+(.*)$` — 1–3 hashes followed by ≥1 whitespace; returns the text
/// after the whitespace run. 4+ hashes or no whitespace → not a heading.
fn heading_text(line: &str) -> Option<&str> {
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    if !(1..=3).contains(&hashes) {
        return None;
    }
    let rest = &line[hashes..];
    let trimmed = rest.trim_start();
    if trimmed.len() == rest.len() {
        return None; // no whitespace after the markers
    }
    Some(trimmed)
}

/// `^---+$` — three or more dashes, nothing else.
fn is_rule(line: &str) -> bool {
    line.len() >= 3 && line.bytes().all(|b| b == b'-')
}

/// Try to match an inline token starting exactly at byte `i`. Precedence order
/// mirrors the TS alternation: **bold**, then `code`, then URL.
fn match_token(line: &str, i: usize, p: &Palette) -> Option<(usize, Span<'static>)> {
    let rest = &line[i..];
    // \*\*[^*]+\*\* — star-free, non-empty content between double stars
    if let Some(inner) = rest.strip_prefix("**")
        && let Some(close) = inner.find("**")
    {
        let content = &inner[..close];
        if !content.is_empty() && !content.contains('*') {
            let end = i + 2 + close + 2;
            return Some((
                end,
                Span::styled(
                    content.to_string(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
            ));
        }
    }
    // `[^`]+` — non-empty content between backticks
    if let Some(inner) = rest.strip_prefix('`')
        && let Some(close) = inner.find('`')
        && close > 0
    {
        let content = &inner[..close];
        let end = i + 1 + close + 1;
        return Some((end, Span::styled(content.to_string(), Style::default().fg(p.info))));
    }
    // https?://[^\s)>\]"']+ — the `+` requires >=1 host char after the scheme.
    let scheme_len = if rest.starts_with("https://") {
        Some(8)
    } else if rest.starts_with("http://") {
        Some(7)
    } else {
        None
    };
    if let Some(scheme_len) = scheme_len {
        let stop = rest
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | '>' | ']' | '"' | '\''))
            .unwrap_or(rest.len());
        if stop > scheme_len {
            return Some((i + stop, Span::styled(rest[..stop].to_string(), Style::default().fg(p.accent))));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(line: &Line) -> Vec<(String, Style)> {
        line.spans
            .iter()
            .map(|s| (s.content.to_string(), s.style))
            .collect()
    }

    fn bold() -> Style {
        Style::default().add_modifier(Modifier::BOLD)
    }
    fn plain() -> Style {
        Style::default()
    }
    fn rule(p: &Palette) -> Style {
        Style::default().fg(p.border)
    }
    fn code(p: &Palette) -> Style {
        Style::default().fg(p.info)
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

    #[test]
    fn bolds_headings_and_strips_markers() {
        let p = Palette::default();
        assert_eq!(parts(&style_line("## Findings", &p)), vec![("Findings".into(), bold())]);
        assert_eq!(parts(&style_line("# Title", &p)), vec![("Title".into(), bold())]);
        assert_eq!(parts(&style_line("### Deep", &p)), vec![("Deep".into(), bold())]);
    }

    #[test]
    fn four_hashes_or_no_space_are_not_headings() {
        let p = Palette::default();
        assert_eq!(parts(&style_line("#### Four", &p)), vec![("#### Four".into(), plain())]);
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
    fn bolds_double_star_spans_and_strips_markers() {
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
    fn colors_inline_code_cyan_and_strips_backticks() {
        let p = Palette::default();
        assert_eq!(
            parts(&style_line("call `foo.py:275` now", &p)),
            vec![
                ("call ".into(), plain()),
                ("foo.py:275".into(), code(&p)),
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
                ("pr.md".into(), code(&p)),
                (" at ".into(), plain()),
                ("https://x.io".into(), link(&p)),
            ]
        );
    }

    #[test]
    fn scheme_only_urls_stay_plain() {
        // TS `https?:\/\/[^\s)>\]"']+` requires >=1 host char after `//`.
        let p = Palette::default();
        assert_eq!(
            parts(&style_line("see http:// done", &p)),
            vec![("see http:// done".into(), plain())]
        );
        assert_eq!(parts(&style_line("https://", &p)), vec![("https://".into(), plain())]);
        assert_eq!(parts(&style_line("http://)", &p)), vec![("http://)".into(), plain())]);
    }

    #[test]
    fn unclosed_bold_stays_plain() {
        let p = Palette::default();
        assert_eq!(
            parts(&style_line("a **b never closes", &p)),
            vec![("a **b never closes".into(), plain())]
        );
    }

    #[test]
    fn returns_one_segment_for_an_empty_line() {
        let p = Palette::default();
        assert_eq!(parts(&style_line("", &p)), vec![("".into(), plain())]);
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

    #[test]
    fn unknown_language_is_plain() {
        let p = Palette::default();
        let got = parts(&style_transcript_line(
            "fn main() {}",
            &LineCtx::Fenced { lang: "rust".into() },
            80,
            &p,
        ));
        assert_eq!(got, vec![("fn main() {}".into(), plain())]);
    }
}
