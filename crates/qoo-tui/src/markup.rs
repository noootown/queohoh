use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::view::theme::Palette;

/// Style one detail-pane line (port of markup.ts styleLine). Whole-line rules
/// (headings, horizontal rules) win; otherwise the line is tokenized into
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
        return Line::from(Span::styled(
            line.to_string(),
            Style::default().add_modifier(Modifier::DIM),
        ));
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
    fn dim() -> Style {
        Style::default().add_modifier(Modifier::DIM)
    }
    fn plain() -> Style {
        Style::default()
    }
    fn code(p: &Palette) -> Style {
        Style::default().fg(p.info)
    }
    fn link(p: &Palette) -> Style {
        Style::default().fg(p.accent)
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
    fn dims_a_horizontal_rule_of_three_or_more_dashes() {
        let p = Palette::default();
        assert_eq!(parts(&style_line("---", &p)), vec![("---".into(), dim())]);
        assert_eq!(parts(&style_line("-----", &p)), vec![("-----".into(), dim())]);
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
}
