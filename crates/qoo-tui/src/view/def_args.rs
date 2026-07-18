//! Two-panel per-def run form. The LEFT panel holds the def's arg fields —
//! drawn by the shared field engine ([`crate::view::form::render_fields`], the
//! same bordered boxes as `Mode::Form`) above a `[ Run ] [ Cancel ]` button
//! row; the RIGHT panel is the def's `prompt.md`, markdown-styled and scrollable
//! exactly like the DETAIL pane's prompt tab. Both panels reuse `menu`'s
//! two-panel [`picker_layout`] + preview helpers, so the run form reads as the
//! same dialog family as the def picker. Consumed by `Mode::DefArgs`.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, BorderType, Borders, Clear};

use crate::hit::{HitMap, HitTarget};
use crate::view::form::{render_fields, render_open_dropdown, FormState};
use crate::view::menu::{picker_layout, render_preview, render_preview_markup, PreviewMetrics};
use crate::view::modal::render_button_row;
use crate::view::theme::Palette;

/// Render the run form: the arg fields (via the shared [`render_fields`]) plus a
/// button row on the LEFT, and the definition's prompt (markdown-styled) on the
/// RIGHT. `prompt` is the cached full def's prompt (`None` → a "loading"
/// placeholder). Registers both panels as `Modal`, one `FormField(i)` per field
/// box, the two `Button` targets, the right panel's `MenuPreview` (wheel
/// scroll), and — when a dropdown is open — its option popup last (topmost).
/// Takes `state` mutably so `render_fields` can cache the focused Textarea's
/// content width. Returns the preview scroll metrics.
pub fn render_def_args(
    frame: &mut Frame,
    hit: &mut HitMap,
    p: &Palette,
    state: &mut FormState,
    def_name: &str,
    prompt: Option<&str>,
    preview_scroll: usize,
) -> PreviewMetrics {
    let layout = picker_layout(frame.area());
    for r in [layout.left, layout.right] {
        frame.render_widget(Clear, r);
        hit.push(r, HitTarget::Modal); // both panels opaque to clicks
    }

    let title_style = Style::default().fg(p.fg).add_modifier(Modifier::BOLD);
    let left_block = Block::default()
        .title(Span::styled(format!(" {def_name} args "), title_style))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(p.accent));
    frame.render_widget(left_block, layout.left);
    let right_block = Block::default()
        .title(Span::styled(format!(" {def_name} "), title_style))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(p.accent));
    frame.render_widget(right_block, layout.right);

    // Left interior: the shared field boxes above a button row on the last line.
    let left_inner = layout.left_inner;
    let fields_area = Rect {
        x: left_inner.x,
        y: left_inner.y,
        width: left_inner.width,
        height: left_inner.height.saturating_sub(1),
    };
    let open_anchor = render_fields(frame, hit, state, fields_area);
    let btn_y = left_inner.y + left_inner.height.saturating_sub(1);
    render_button_row(
        frame,
        hit,
        Rect { x: left_inner.x, y: btn_y, width: left_inner.width, height: 1 },
        &state.primary_label,
        state.button_focus(),
        p.accent,
    );

    // Right panel: the def's prompt, markdown-styled like the DETAIL prompt tab
    // (or a plain placeholder until the definition arrives).
    let metrics = match prompt {
        Some(md) => render_preview_markup(frame, hit, &layout, &[], md, preview_scroll),
        None => render_preview(
            frame,
            hit,
            &layout,
            &[("(loading definition…)".to_string(), p.dim_style())],
            preview_scroll,
        ),
    };

    // Open dropdown popup last so it is topmost (Clear + `DropdownItem` clicks win).
    if let Some((anchor, options)) = open_anchor {
        render_open_dropdown(frame, hit, state, frame.area(), anchor, options);
    }
    metrics
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::form::{Field, FormState};
    use ratatui::{backend::TestBackend, Terminal};

    fn render(state: &mut FormState, prompt: Option<&str>, cols: u16, rows: u16) -> (String, HitMap) {
        let p = Palette::default();
        let mut term = Terminal::new(TestBackend::new(cols, rows)).unwrap();
        let mut hit = HitMap::default();
        term.draw(|f| {
            render_def_args(f, &mut hit, &p, state, "pr-ready", prompt, 0);
        })
        .unwrap();
        let buf = term.backend().buffer().clone();
        let mut s = String::new();
        for y in 0..rows {
            for x in 0..cols {
                s.push_str(buf[(x, y)].symbol());
            }
            s.push('\n');
        }
        (s, hit)
    }

    #[test]
    fn run_button_click_hits_confirm_for_a_four_field_def() {
        // Repro of the pr-review shape: model "" head + a required worktree
        // combobox + two enum dropdowns (4 fields after the model-first reorder).
        // Clicking `[ Run ]` must resolve to Button(Confirm), not the inert Modal.
        use crate::view::form::DropdownOption;
        let mut model = Field::dropdown_labeled(
            "model",
            vec![
                DropdownOption { value: String::new(), label: "default (grok-4.5)".into() },
                DropdownOption { value: "grok/grok-4.5".into(), label: "grok-4.5 (grok)".into() },
            ],
            "",
        );
        model.required = false;
        let mut target = Field::combobox("target", vec![], "pr:1762");
        target.required = true;
        let mut state = FormState::new(
            "pr-review",
            "Run",
            vec![
                model,
                target,
                Field::dropdown("static_review", vec!["false".into(), "true".into()], "false"),
                Field::dropdown("e2e_review", vec!["true".into(), "false".into()], "true"),
            ],
        );
        let (s, hit) = render(&mut state, Some("Review the PR."), 100, 30);
        // Locate `[ Run ]` and probe the middle of "Run".
        let (rx, ry) = {
            let mut found = None;
            for (y, line) in s.lines().enumerate() {
                if let Some(cx) = line.find("[ Run ]") {
                    found = Some((cx + 3, y)); // the 'R'
                    break;
                }
            }
            found.expect("[ Run ] button rendered")
        };
        assert!(
            matches!(hit.hit(rx as u16, ry as u16), Some(HitTarget::Button(crate::hit::ButtonKind::Confirm))),
            "a click on [ Run ] must hit Button(Confirm), got {:?}",
            hit.hit(rx as u16, ry as u16)
        );
    }

    #[test]
    fn renders_fields_button_row_and_prompt() {
        let mut state = FormState::new(
            "pr-ready",
            "Run",
            vec![
                Field::dropdown("review", vec!["full-review".into(), "bypass-review".into()], "full-review"),
                Field::input("pr", "", true),
            ],
        );
        let (s, hit) = render(&mut state, Some("Ready a PR for review."), 100, 24);
        assert!(s.contains("pr-ready args"), "left panel titled with the def");
        assert!(s.contains("review"));
        assert!(s.contains("pr"));
        assert!(s.contains('▾'), "dropdown chevron renders");
        assert!(s.contains("[ Run ]") && s.contains("[ Cancel ]"), "button row present");
        assert!(s.contains("Ready a PR for review"), "prompt renders on the right");
        // Field boxes + both panels register hit targets.
        let (mut f0, mut f1, mut preview) = (false, false, false);
        for y in 0..24 {
            for x in 0..100 {
                match hit.hit(x, y) {
                    Some(HitTarget::FormField(0)) => f0 = true,
                    Some(HitTarget::FormField(1)) => f1 = true,
                    Some(HitTarget::MenuPreview) => preview = true,
                    _ => {}
                }
            }
        }
        assert!(f0 && f1 && preview, "field boxes + preview register hit targets");
    }

    #[test]
    fn tall_textarea_does_not_clip_siblings_or_button_row() {
        // Carry-forward rule (Phase 1 auto-grow): in a MULTI-field form the
        // textarea grows only into the space left after the other fields + the
        // button row, bounded by the fixed left panel — so a value far taller
        // than the panel never clips a sibling field or the buttons.
        let mut tall = String::new();
        for i in 0..40 {
            tall.push_str(&format!("line {i}\n"));
        }
        let mut state = FormState::new(
            "pr-ready",
            "Run",
            vec![
                Field::dropdown("review", vec!["full-review".into(), "bypass-review".into()], "full-review"),
                Field::input("pr", "45", true),
                Field::textarea("notes", &tall, false),
            ],
        );
        state.focus_field(2); // focus the tall textarea
        let (s, _hit) = render(&mut state, Some("prompt"), 100, 24);
        assert!(s.contains("review"), "first field survives a tall textarea: {s}");
        assert!(s.contains("notes"), "the textarea field itself survives");
        assert!(s.contains("[ Run ]"), "the button row is never clipped by growth");
        assert!(s.contains("[ Cancel ]"), "the full button row survives");
    }

    #[test]
    fn def_args_two_panel_snapshot() {
        let mut state = FormState::new(
            "pr-ready",
            "Run",
            vec![
                Field::dropdown("review", vec!["full-review".into(), "bypass-review".into()], "full-review"),
                Field::input("pr", "45", true),
            ],
        );
        let (s, _hit) = render(&mut state, Some("Ready a PR for review.\n\nDetect the PR, run checks."), 100, 24);
        insta::assert_snapshot!("def_args_two_panel", s);
    }
}
