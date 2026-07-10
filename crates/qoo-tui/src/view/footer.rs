use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;

use crate::app::{App, PaneId};
use crate::view::theme::Palette;
use crate::view::{Computed, selection_range};

/// Style a hint string so the key tokens stand out instead of the whole line
/// rendering dim: `[key]` chunks get the accent color (bold), `·` separators
/// stay dim, and the remaining label text uses the normal foreground.
fn hint_line(s: &str, p: &Palette) -> Line<'static> {
    let key_style = Style::default().fg(p.accent).add_modifier(Modifier::BOLD);
    let label_style = Style::default().fg(p.fg);
    let mut spans: Vec<Span> = Vec::new();
    let mut label = String::new();
    let mut key: Option<String> = None;
    for ch in s.chars() {
        match (ch, &mut key) {
            ('[', None) => {
                if !label.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut label), label_style));
                }
                key = Some(String::new());
            }
            (']', Some(k)) => {
                spans.push(Span::styled(format!("[{k}]"), key_style));
                key = None;
            }
            ('·', None) => {
                if !label.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut label), label_style));
                }
                spans.push(Span::styled("·".to_string(), p.dim_style()));
            }
            (_, Some(k)) => k.push(ch),
            (_, None) => label.push(ch),
        }
    }
    // Unterminated `[...` (never in practice) renders literally as label text.
    if let Some(k) = key {
        label.push('[');
        label.push_str(&k);
    }
    if !label.is_empty() {
        spans.push(Span::styled(label, label_style));
    }
    Line::from(spans)
}

/// The single global-key footer line. Pane-scoped actions (new/actions/collapse)
/// now live on the pane title bars, so the footer only lists keys that are global
/// regardless of focus.
const GLOBAL_HINT: &str =
    "[1-9/0 · ctrl+s n/p] tab · [ctrl+x/z] sub-tab · [?] help · [q]uit";

pub fn render(app: &App, c: &Computed, frame: &mut ratatui::Frame, area: Rect) {
    let p: &Palette = &c.palette;
    // Priority: armed prefix > searching > status line > selection-count > global.
    // The armed `ctrl+s` prefix takes the line so its awaiting-n/p state is obvious.
    if app.prefix_armed {
        frame.render_widget(
            Paragraph::new(hint_line(
                "prefix [ctrl+s] · [n] next tab · [p] prev tab · any other key cancels",
                p,
            )),
            area,
        );
        return;
    }
    let searching = c.searching.iter().any(|&s| s);
    if searching {
        frame.render_widget(
            Paragraph::new(hint_line("type to filter · [enter] apply · [esc] clear", p)),
            area,
        );
        return;
    }
    if let Some(status) = &app.status_line {
        frame.render_widget(
            Paragraph::new(Text::from(status.clone())).style(Style::default().fg(p.error)),
            area,
        );
        return;
    }
    // Selection count of the focused list pane.
    let sel = match c.ui.focus {
        PaneId::Queue => Some((&c.queue_sel, c.queue.len())),
        PaneId::Tasks => Some((&c.tasks_sel, c.defs.len())),
        PaneId::Worktrees => Some((&c.wt_sel, c.worktrees.len())),
        PaneId::Detail => None,
    };
    let count = sel
        .filter(|(_, len)| *len > 0)
        .map(|(s, _)| {
            let (a, b) = selection_range(s);
            b - a + 1
        })
        .unwrap_or(0);
    if count > 1 {
        frame.render_widget(
            Paragraph::new(hint_line(
                &format!("{count} selected · [a] bulk actions · [shift+↑↓] extend · [esc] clear"),
                p,
            )),
            area,
        );
        return;
    }
    frame.render_widget(Paragraph::new(hint_line(GLOBAL_HINT, p)), area);
}
