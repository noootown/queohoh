use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Text;
use ratatui::widgets::Paragraph;

use crate::app::{App, PaneId};
use crate::view::theme::Palette;
use crate::view::{Computed, selection_range};

const LIST_HINT: &str =
    "[a] actions · [enter] detail · [↑↓] move · [/] filter · [?] help · [q]uit";

fn hint_for(focus: PaneId) -> String {
    match focus {
        PaneId::Queue => format!("[c] new run · {LIST_HINT}"),
        PaneId::Tasks => LIST_HINT.to_string(),
        PaneId::Worktrees => format!("[c] new worktree · {LIST_HINT}"),
        PaneId::Detail => {
            "[↑↓/jk] scroll · [g/G] top/bottom · [{ }] sub-tab · [a] actions · [?] help · [q]uit"
                .to_string()
        }
    }
}

pub fn render(app: &App, c: &Computed, frame: &mut ratatui::Frame, area: Rect) {
    let p: &Palette = &c.palette;
    // Priority: searching > status line > selection-count > per-pane hints.
    // `searching` is all-false until Task 11 wires `Mode::Search`.
    let searching = c.searching.iter().any(|&s| s);
    if searching {
        frame.render_widget(
            Paragraph::new("type to filter · [enter] apply · [esc] clear").style(p.dim_style()),
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
            Paragraph::new(format!(
                "{count} selected · [a] bulk actions · [shift+↑↓] extend · [esc] clear"
            ))
            .style(p.dim_style()),
            area,
        );
        return;
    }
    frame.render_widget(Paragraph::new(hint_for(c.ui.focus)).style(p.dim_style()), area);
}
