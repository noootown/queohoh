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
        // Key-column padding survives the wrap (generic word_wrap would collapse
        // the multi-space gap between `path` and `/Users…`).
        assert!(
            display[0].text.starts_with("path     /"),
            "first segment must keep key-column padding: {:?}",
            display[0].text
        );
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
    fn wrapped_config_discovery_row_does_not_paint_value_as_key() {
        // Regression for the definition config tab: a long `discovery` value
        // (shell command + item_key) used to go through generic word_wrap, which
        // collapsed `discovery         bash…` → `discovery bash…`. style_config_line
        // then painted the first key_col chars — including ` bash tas…` — accent
        // cyan. Operators saw "discovery bash tasks" all in key color.
        let p = Palette::default();
        let key_col = 18; // "purge_after_days" (16) + CONFIG_KEY_GAP (2)
        let key = format!("{:<key_col$}", "discovery");
        let value = "bash tasks/pr-fix-ci-conflicts/discover.sh {{github_username}} {{platform_repo}}  ·  item_key: {{url}}@{{head_sha}}";
        let line = format!("{key}{value}");
        assert!(
            line.chars().count() > key_col + 5,
            "fixture must be long enough to exercise wrap"
        );
        let display = wrap_lines(
            &[line],
            &[LineCtx::Config { key_col }],
            40, // force wrap well under the full line width
        );
        assert!(display.len() > 1, "long discovery value must wrap: {display:?}");

        // First segment: exact key+padding prefix, then a non-empty value start.
        assert!(
            display[0].text.starts_with(&key),
            "key-column padding must survive wrap: {:?}",
            display[0].text
        );
        assert_eq!(
            display[0].ctx,
            LineCtx::Config { key_col },
            "first segment keeps the real key_col"
        );
        let first = parts(&style_transcript_line(
            &display[0].text,
            &display[0].ctx,
            40,
            &p,
        ));
        assert_eq!(first[0], (key.clone(), accent(&p)), "only the key column is accent");
        // Value start must not share the accent style.
        assert!(
            first.iter().skip(1).all(|(_, st)| *st != accent(&p)),
            "value spans must not be accent: {first:?}"
        );
        assert!(
            first.iter().skip(1).any(|(t, _)| t.starts_with("bash")),
            "value should still start with the command: {first:?}"
        );

        // Continuations hang under the value column (indent = key_col spaces) and
        // style wholly as value (key_col: 0).
        for seg in &display[1..] {
            assert_eq!(seg.ctx, LineCtx::Config { key_col: 0 });
            assert!(
                seg.text.starts_with(&" ".repeat(key_col)),
                "continuation should indent under the value column: {:?}",
                seg.text
            );
            let styled = parts(&style_transcript_line(&seg.text, &seg.ctx, 40, &p));
            assert!(
                styled.iter().all(|(_, st)| *st != accent(&p)),
                "continuation must not re-color as key: {:?} → {styled:?}",
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
