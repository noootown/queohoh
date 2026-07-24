/// Minimum rows any expanded left pane keeps (border + title-on-border + two
/// content rows). With the title now embedded in the top border, four rows still
/// leaves two rows of content.
pub const PANE_MIN_H: u16 = 4;

/// Rows a collapsed pane occupies: just the title border row and the bottom
/// border. No content, no scrollbar.
pub const COLLAPSED_H: u16 = 2;

/// Heights of the three stacked left panes. Pure and re-clamped every frame so
/// session drag overrides never violate the invariants: every EXPANDED pane
/// ≥ `PANE_MIN_H`, every COLLAPSED pane is pinned to `COLLAPSED_H`, and the three
/// sum to exactly `body_height` (the last expanded pane — or, when all three are
/// collapsed, the last pane — absorbs the remainder).
///
/// With no pane collapsed and both overrides `None` this reproduces the historic
/// 2:1:1 default exactly (so default snapshots never move). `queue_h`/`tasks_h`
/// overrides are the requested heights from a divider drag; they are clamped, not
/// trusted. `collapsed` is `[queue, tasks, worktrees]`.
pub fn pane_layout(
    body_height: u16,
    queue_h: Option<u16>,
    tasks_h: Option<u16>,
    collapsed: [bool; 3],
) -> PaneLayout {
    const MIN: u16 = PANE_MIN_H;
    const COL: u16 = COLLAPSED_H;

    // No pane collapsed → the historic formula, byte-for-byte (default snapshots
    // and every legacy override test stay pinned).
    if collapsed == [false, false, false] {
        // Below room for three floors nobody can be satisfied; hand each the floor
        // and let the view's Length constraints clamp the overflow. Matches the old
        // default, which also produced (MIN,MIN,MIN) for any body ≤ 3·MIN.
        if body_height <= 3 * MIN {
            return PaneLayout { queue_h: MIN, tasks_h: MIN, worktrees_h: MIN };
        }
        // Default 2:1:1 heights, used for whichever override is absent.
        let def_tasks = std::cmp::max(MIN, body_height / 4);
        let def_queue = std::cmp::max(MIN, body_height.saturating_sub(2 * def_tasks));
        // Clamp queue into [MIN, body − 2·MIN] (leaves a floor each for tasks +
        // worktrees), then tasks into [MIN, body − queue − MIN] (leaves a floor for
        // worktrees). worktrees takes whatever is left — always ≥ MIN by construction.
        let q = queue_h.unwrap_or(def_queue).clamp(MIN, body_height - 2 * MIN);
        let t = tasks_h.unwrap_or(def_tasks).clamp(MIN, body_height - q - MIN);
        let w = body_height - q - t;
        return PaneLayout { queue_h: q, tasks_h: t, worktrees_h: w };
    }

    // Collapse-aware allocation. Collapsed panes are pinned to COL rows; the
    // expanded panes share what remains, each ≥ MIN, and the three heights sum to
    // exactly body_height.
    let ncol = collapsed.iter().filter(|&&c| c).count() as u16;
    let mut h = [0u16; 3];
    for (i, &c) in collapsed.iter().enumerate() {
        if c {
            h[i] = COL;
        }
    }
    let avail = body_height.saturating_sub(ncol * COL);
    let expanded: Vec<usize> = (0..3).filter(|&i| !collapsed[i]).collect();
    match expanded.as_slice() {
        // All three collapsed: no content pane. The leftover becomes a blank
        // filler region folded into the last pane's allocation (the collapsed bar
        // still renders only COL rows at its top, leaving the rest blank).
        [] => h[2] = h[2].saturating_add(avail),
        // One expanded pane takes everything left.
        [a] => h[*a] = std::cmp::max(MIN, avail),
        // Two expanded panes split `avail`: the upper honors its override (or an
        // even split), the lower absorbs the remainder. The upper index is always
        // 0 or 1, so it always has an override field; worktrees (index 2) is only
        // ever the lower of a pair.
        [a, b] => {
            let ov = match *a {
                0 => queue_h,
                1 => tasks_h,
                _ => None,
            };
            let hi = avail.saturating_sub(MIN).max(MIN);
            let ha = ov.unwrap_or(avail / 2).clamp(MIN, hi);
            h[*a] = ha;
            h[*b] = avail.saturating_sub(ha);
        }
        _ => unreachable!("at least one pane is collapsed in this branch"),
    }
    PaneLayout { queue_h: h[0], tasks_h: h[1], worktrees_h: h[2] }
}

/// Clamp a requested left-column width so both sides stay usable: left keeps
/// `MIN_LEFT`, DETAIL keeps `MIN_RIGHT`. The `.max(MIN_LEFT)` on the ceiling keeps
/// the range non-empty (so `clamp` never panics) even at the 60-col minimum.
pub fn clamp_left_cols(total_width: u16, want: u16) -> u16 {
    const MIN_LEFT: u16 = 24;
    const MIN_RIGHT: u16 = 30;
    let hi = total_width.saturating_sub(MIN_RIGHT).max(MIN_LEFT);
    want.clamp(MIN_LEFT, hi)
}

/// Cursor-centered scroll window: half-open `(start, end)` slice indices of the
/// visible rows (`start` is the TS `offset`).
pub fn window_rows(len: usize, cursor: usize, capacity: usize) -> (usize, usize) {
    if capacity == 0 || len == 0 {
        return (0, 0);
    }
    if len <= capacity {
        return (0, len);
    }
    let clamped = cursor.min(len - 1);
    let start = clamped.saturating_sub(capacity / 2).min(len - capacity);
    (start, start + capacity)
}

/// The pane's border title: the base plus a `· N selected` suffix when the pane
/// holds a BULK selection. `selected` is the union count (range ∪ marks) the
/// caller resolved via `view::selected_positions`; `bulk` is
/// `view::is_bulk_selection`. Both are passed in rather than derived here: a
/// mark-aware count needs the pane's rows, which this pure helper doesn't see.
/// The `/filter` + cursor decoration lives in the inline hint row (see
/// `view::panes`), so it is not part of the title.
///
/// `bulk` and `selected` can disagree: a mark is `bulk` by presence even when
/// it resolves to no visible row (e.g. filtered out by search), in which case
/// `selected` is 0. The suffix only renders when `selected > 0` — "· 0
/// selected" would be nonsensical, and the status line already explains any
/// blocked action.
pub fn pane_title(base: &str, selected: usize, bulk: bool) -> String {
    if bulk && selected > 0 {
        format!("{base} · {selected} selected")
    } else {
        base.to_string()
    }
}

/// The QUEUE pane's title-bar summary: outstanding work at a glance —
/// `N queued · N running` (counts over the pane's rows, so an active filter
/// summarizes what is shown).
pub fn queue_pane_summary(rows: &[QueueRow]) -> String {
    let queued = rows.iter().filter(|r| r.status == TaskStatus::Queued).count();
    let running = rows.iter().filter(|r| r.running).count();
    format!("{queued} queued · {running} running")
}

/// The TASKS pane's title-bar summary: the definition count — `N tasks`.
pub fn tasks_pane_summary(defs: &[DefinitionSummary]) -> String {
    let n = defs.len();
    if n == 1 { "1 task".to_string() } else { format!("{n} tasks") }
}

/// The WORKTREES pane's title-bar summary: `N busy · N total`. Busy = a task is
/// running on the lane; total counts real worktrees only (session rows are not
/// worktrees).
pub fn wt_pane_summary(rows: &[WorktreeRow]) -> String {
    let busy = rows.iter().filter(|r| r.running_elapsed.is_some()).count();
    let total = rows.iter().filter(|r| !r.is_session).count();
    format!("{busy} busy · {total} total")
}

/// Indices of rows whose text matches the filter (case-insensitive substring;
/// empty filter matches everything).
pub fn filter_rows<T>(rows: &[T], filter: &str, text_of: impl Fn(&T) -> String) -> Vec<usize> {
    if filter.is_empty() {
        return (0..rows.len()).collect();
    }
    let needle = filter.to_lowercase();
    rows.iter()
        .enumerate()
        .filter(|(_, row)| text_of(row).to_lowercase().contains(&needle))
        .map(|(i, _)| i)
        .collect()
}

/// Haystack for QUEUE pane `/` search. Includes definition name (task name),
/// worktree, and prompt summary so a query like `intake` or `pr-ready` or a
/// worktree `JUS-1966` hits the right rows — not just freeform prompt text.
pub fn queue_search_text(row: &QueueRow) -> String {
    let mut s = String::with_capacity(
        row.def_name.as_ref().map(|d| d.len() + 1).unwrap_or(0)
            + row.worktree.len()
            + 1
            + row.summary.len(),
    );
    if let Some(d) = &row.def_name {
        s.push_str(d);
        s.push(' ');
    }
    if !row.worktree.is_empty() {
        s.push_str(&row.worktree);
        s.push(' ');
    }
    s.push_str(&row.summary);
    s
}

/// "pr, mode=ready, review=auto" — `name` for required args, `name=default` otherwise.
pub fn arg_summary(args: &[ArgSpec]) -> String {
    args.iter()
        .map(|a| match &a.default {
            Some(d) => format!("{}={}", a.name, d),
            None => a.name.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Sentinel worktree name for a task targeting the project's primary checkout
/// (mirror of core's `REPO_SENTINEL`). Never a real worktree — display it as
/// the bare repo name, matching how the primary checkout appears elsewhere.
pub const REPO_SENTINEL: &str = "@repo";

pub fn strip_repo_prefix<'a>(worktree: &'a str, repo: &'a str) -> &'a str {
    if worktree == REPO_SENTINEL {
        return repo;
    }
    match worktree.strip_prefix(repo) {
        Some(rest) => match rest.strip_prefix('.') {
            Some(stripped) => stripped,
            None => worktree, // bare repo name or shared prefix without the dot
        },
        None => worktree,
    }
}

pub fn lane_key(repo: &str, worktree: &str) -> String {
    format!("{repo}:{worktree}")
}

/// First non-blank line of the prompt, trimmed, clipped to ≤240 chars with `…`.
/// The generous cap only bounds pathological one-line prompts — the queue's
/// summary column does the real width-fitting per frame, so the summary can
/// flex across however much row the pane has.
pub fn prompt_summary(prompt: &str) -> String {
    let line = prompt
        .lines()
        .find(|l| !l.trim().is_empty())
        .map(str::trim)
        .unwrap_or("");
    let chars: Vec<char> = line.chars().collect();
    if chars.len() <= 240 {
        return line.to_string();
    }
    let mut out: String = chars[..239].iter().collect();
    out.push('…');
    out
}

/// Running elapsed prefix — stopwatch clock (`⏱`), must match
/// `view::theme::GLYPH_TIMER`. Distinct from the defer countdown hourglass.
const TIMER_GLYPH: char = '⏱';
/// Deferred countdown prefix — single-width hourglass (`⧗`), must match
/// `view::theme::GLYPH_DEFER` / `GLYPH_TIMED_OUT`. Not the emoji ⏳.
const DEFER_GLYPH: char = '⧗';

/// "`⏱ 47s`" / "`⏱ 5m03s`" (zero-padded seconds) / "`⏱ 1h02m`" (zero-padded
/// minutes). Running elapsed uses the clock glyph; deferred countdown uses
/// [`remaining_label`]'s hourglass.
pub fn elapsed_label(created_epoch_s: u64, now_epoch_s: u64) -> String {
    let total = now_epoch_s.saturating_sub(created_epoch_s);
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{TIMER_GLYPH} {hours}h{minutes:02}m")
    } else if minutes > 0 {
        format!("{TIMER_GLYPH} {minutes}m{seconds:02}s")
    } else {
        format!("{TIMER_GLYPH} {seconds}s")
    }
}

/// Countdown for a deferred task's `notBefore`: `⧗ 4h32m` / `⧗ 12m` / `⧗ 45s`.
/// Hourglass prefix ([`DEFER_GLYPH`]), distinct from running's stopwatch clock.
/// Clamps a past/equal `until` to `⧗ 0s` (caller normally falls back to
/// `#N in lane` once due). Width stays within [`QUEUE_LIVE_W`] for single-
/// digit hours; multi-digit hours clip at paint.
pub fn remaining_label(until_epoch_s: u64, now_epoch_s: u64) -> String {
    let total = until_epoch_s.saturating_sub(now_epoch_s);
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{DEFER_GLYPH} {hours}h{minutes:02}m")
    } else if minutes > 0 {
        format!("{DEFER_GLYPH} {minutes}m")
    } else {
        format!("{DEFER_GLYPH} {seconds}s")
    }
}

/// Epoch (seconds) the task's CURRENT run started: its `started_at` (re-stamped
/// on every (re-)run when the worker flips it to `running`), falling back to
/// `created` when absent — a task that never ran, or an old daemon that omits the
/// field. Anchoring the live `⏱` timer here means a re-run's clock restarts from
/// the re-run, not the original creation — so a re-run doesn't inherit hours of
/// phantom elapsed and read as if it were about to hit the 3h wall-clock ceiling.
fn run_start_epoch_s(task: &TaskInstance) -> u64 {
    parse_iso_epoch_s(task.started_at.as_deref().unwrap_or(&task.created))
}

/// Parse a daemon ISO-8601 UTC timestamp ("YYYY-MM-DDTHH:MM:SS[.mmm]Z") into
/// epoch seconds. No date crate: Howard Hinnant's days-from-civil algorithm.
pub fn parse_iso_epoch_s(iso: &str) -> u64 {
    if iso.len() < 19 {
        return 0;
    }
    let num = |s: &str| s.parse::<i64>().unwrap_or(0);
    let (y, m, d) = (num(&iso[0..4]), num(&iso[5..7]), num(&iso[8..10]));
    let (hh, mm, ss) = (num(&iso[11..13]), num(&iso[14..16]), num(&iso[17..19]));
    let secs = days_from_civil(y, m, d) * 86_400 + hh * 3600 + mm * 60 + ss;
    if secs < 0 { 0 } else { secs as u64 }
}

/// Days since 1970-01-01 for a proleptic-Gregorian civil date
/// (Howard Hinnant's `days_from_civil`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Inverse of `days_from_civil`: (year, month, day) for a days-since-epoch count
/// (Howard Hinnant's `civil_from_days`).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// "MM/DD HH:MM" in local time. `utc_offset_s` is injected so tests are
/// deterministic (production passes the real local offset).
pub fn absolute_local_label(created_epoch_s: u64, utc_offset_s: i32) -> String {
    let local = created_epoch_s as i64 + utc_offset_s as i64;
    let days = local.div_euclid(86_400);
    let secs = local.rem_euclid(86_400);
    let (_, m, d) = civil_from_days(days);
    let hh = secs / 3600;
    let mm = (secs % 3600) / 60;
    format!("{m:02}/{d:02} {hh:02}:{mm:02}")
}

/// "just now" / "5m ago" / "1h ago" / "2d ago".
pub fn relative_age_label(created_epoch_s: u64, now_epoch_s: u64) -> String {
    let delta = now_epoch_s.saturating_sub(created_epoch_s);
    if delta < 60 {
        "just now".to_string()
    } else if delta < 3600 {
        format!("{}m ago", delta / 60)
    } else if delta < 86_400 {
        format!("{}h ago", delta / 3600)
    } else {
        format!("{}d ago", delta / 86_400)
    }
}

// ---- cron humanizer (pure) -----------------------------------------------------

/// `HH:MM` on a 12-hour clock with an `am`/`pm` suffix; the `:MM` is dropped when
/// the minute is zero. e.g. `(30, 13) → "1:30pm"`, `(0, 9) → "9am"`, `(0, 0) → "12am"`.
fn fmt_time(min: u32, hour: u32) -> String {
    let ampm = if hour < 12 { "am" } else { "pm" };
    let h12 = match hour % 12 {
        0 => 12,
        h => h,
    };
    if min == 0 {
        format!("{h12}{ampm}")
    } else {
        format!("{h12}:{min:02}{ampm}")
    }
}

/// Abbreviated weekday for a cron day-of-week number (0 or 7 == Sunday).
fn day_name(d: u32) -> &'static str {
    match d % 7 {
        0 => "Sun",
        1 => "Mon",
        2 => "Tue",
        3 => "Wed",
        4 => "Thu",
        5 => "Fri",
        _ => "Sat",
    }
}

/// `1 → "1st"`, `2 → "2nd"`, `3 → "3rd"`, `11..=13 → "…th"`, else `"…th"`.
fn ordinal(n: u32) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// A cron field made only of the characters a standard schedule uses.
fn is_cron_field(f: &str) -> bool {
    !f.is_empty()
        && f.chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '*' | '/' | ',' | '-'))
}

/// Parse a field that must be a single integer in `[lo, hi]`; `None` otherwise
/// (a range/step/list is not a single value).
fn single(f: &str, lo: u32, hi: u32) -> Option<u32> {
    let v: u32 = f.parse().ok()?;
    (lo..=hi).contains(&v).then_some(v)
}

/// The `N` of a `*/N` step field, or `None` when the field isn't a step.
fn step(f: &str) -> Option<u32> {
    f.strip_prefix("*/").and_then(|s| s.parse().ok())
}

/// A comma list whose members are exactly {Sat, Sun} (any 0/6/7 spelling).
fn is_weekend(dow: &str) -> bool {
    let mut days: Vec<u32> = dow
        .split(',')
        .filter_map(|s| s.parse::<u32>().ok())
        .map(|d| d % 7)
        .collect();
    days.sort_unstable();
    days.dedup();
    days == [0, 6]
}

/// Best-effort humanization of the five parsed cron fields. Returns `None` when
/// the pattern is a valid cron shape we don't confidently phrase — `cron_human`
/// then falls back to the raw expression rather than dropping it.
fn humanize_fields(f: &[&str; 5]) -> Option<String> {
    let [m, h, dom, mon, dow] = *f;
    let all_dmw = dom == "*" && mon == "*" && dow == "*";

    // Frequency tiers: minute/hour carry a step or the top-of-hour marker. A
    // tuple match keeps the arms flat (nested ifs here would be collapsible).
    if all_dmw {
        match (step(m), step(h), m, h) {
            (Some(n), _, _, "*") => return Some(format!("Every {n}m")),
            (_, _, "0", "*") => return Some("Hourly".to_string()),
            (_, Some(n), "0", _) => return Some(format!("Every {n}h")),
            _ => {}
        }
    }

    // Time-of-day tiers need a concrete minute + hour.
    let time = fmt_time(single(m, 0, 59)?, single(h, 0, 23)?);

    if dom == "*" && mon == "*" {
        if dow == "*" {
            return Some(format!("Everyday {time}"));
        }
        if dow == "1-5" {
            return Some(format!("Weekdays {time}"));
        }
        if is_weekend(dow) {
            return Some(format!("Weekends {time}"));
        }
        if let Some(d) = single(dow, 0, 7) {
            return Some(format!("{} {time}", day_name(d)));
        }
        return None; // an unhandled day-of-week list → raw fallback
    }

    if mon == "*" && dow == "*" {
        return single(dom, 1, 31).map(|d| format!("Monthly {} {time}", ordinal(d)));
    }

    None
}

/// Turn a standard 5-field cron expression into a short human phrase for the
/// TASKS schedule column. Best-effort: recognized patterns get a friendly phrase
/// (`"30 13 * * *" → "Everyday 1:30pm"`), any other valid-shaped cron falls back
/// to the raw expression, and empty/non-cron input returns `None` (showing
/// nothing beats noise). See the unit tests for the full tier table.
pub fn cron_human(expr: &str) -> Option<String> {
    let expr = expr.trim();
    if expr.is_empty() {
        return None;
    }
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 || !fields.iter().all(|f| is_cron_field(f)) {
        return None; // not a standard 5-field cron
    }
    let five = [fields[0], fields[1], fields[2], fields[3], fields[4]];
    Some(humanize_fields(&five).unwrap_or_else(|| expr.to_string()))
}

// ---- column layout (pure, per-frame, computed from the VISIBLE rows) ----------
//
// Every content glyph the list rows use (▶ ✓ ✗ ○ ? · ⛓ ⧗ ◆ ●) measures one
// terminal cell, so column widths can be reasoned about in chars. Truncation is
// char-based (never byte slicing) so unicode text can't panic.

/// Char count of `s` (== cell width for the row content we render).
fn cw(s: &str) -> usize {
    s.chars().count()
}

/// Clip `s` to `width` chars, appending `…` when truncated (mirrors
/// `prompt_summary`). `width == 0` yields the empty string.
pub fn clip(s: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= width {
        return s.to_string();
    }
    if width == 1 {
        return "…".to_string();
    }
    let mut out: String = chars[..width - 1].iter().collect();
    out.push('…');
    out
}

/// Left-align `s` in a `width`-char field: clip if too long, right-pad with
/// spaces if short. The result is always exactly `width` chars.
pub fn pad_clip(s: &str, width: usize) -> String {
    let mut out = clip(s, width);
    let n = out.chars().count();
    if n < width {
        out.extend(std::iter::repeat_n(' ', width - n));
    }
    out
}

/// Largest char-width among `values`, capped at `cap` (0 if the iterator is
/// empty). Used to size the name/worktree/def columns to the widest visible cell.
fn capped_max<'a>(values: impl Iterator<Item = &'a str>, cap: usize) -> usize {
    values.map(cw).max().unwrap_or(0).min(cap)
}

/// Content cap for identity name columns across panes: QUEUE worktree, WORKTREES
/// worktree name, and TASKS def name. Sized so long names leave room for trailing
/// columns (summary / last-task / author / PR / model).
pub const WORKTREE_CAP: usize = 28;
pub const DEF_CAP: usize = 20;
/// Max width of the humanized schedule text in the TASKS pane. A raw-cron
/// fallback longer than this is clipped with `…` rather than blowing out the
/// row.
pub const SCHED_CAP: usize = 20;
/// Max width of the model cell in the TASKS pane (effective head label only,
/// e.g. `claude-opus-4.8` / `grok-4.5`). Clipped with `…` if longer; the column
/// still degrades (drops) before the name shrinks.
pub const MODEL_CAP: usize = 20;
pub const SUMMARY_MIN: usize = 10;
/// Gutter between adjacent field columns (glyph/chain markers keep single
/// spaces; the field columns get a wider gap so they read as columns).
pub const COL_GAP: usize = 2;
/// Fixed width of the absolute timestamp column (`MM/DD HH:MM`).
pub const TIMESTAMP_W: usize = 11;

// ---- FIXED reserved widths for the metadata/marker/time/live columns --------
//
// Column PRESENCE is a function of the pane width (and, for capability columns,
// whole-pane data availability) — never of an individual row's data. A row that
// lacks a value renders blanks (`pad_clip("", W)`) in its reserved cell, so a
// timer appearing or a wide value scrolling in never shifts any other column.
// Values fit the realistic max label under `cw`.

/// Live timer column (`⧗ 99h59m`: single-width hourglass + space + up-to-2-digit hours).
pub const TIMER_W: usize = 8;
/// Relative-age column (`relative_age_label` max is `just now` = 8).
pub const AGE_W: usize = 8;
/// Last-commit author column (fixed reserved width; longer names clip with `…`).
pub const AUTHOR_W: usize = 14;
/// Last-commit relative-age column (`relative_age_label` max `just now`).
pub const COMMIT_AGE_W: usize = 8;
/// Open-PR column (`#<n>`): a fixed reserved width like author/commit-age.
/// Sized for a 5-digit PR number plus the `#` (`#12345`); longer numbers clip.
pub const PR_W: usize = 6;
/// Shared QUEUE live slot: `⧗ 99h59m` (8), `#N in lane` (`#9 in lane` = 10),
/// or a deferred countdown (`⧗ 4h32m` = 8). Sized to the widest common label
/// (`#9 in lane` / multi-digit hour timers clip).
pub const QUEUE_LIVE_W: usize = 10;
/// Fixed reserved width of the worktrees combined Next/Live activity column.
/// One trailing slot holds either `⏱ <elapsed>`, `→ <name>`, or both
/// (`⧗ … → …`). The queued COUNT is not shown here — the leading indicator
/// digit already carries it. Static 20 cells (user request) so the column never
/// grows/shrinks with content; longer names clip with `…`.
const WT_ACTIVITY_W: usize = 20;
/// Floor of the worktrees last-task FILL column (the pane's flex column — it
/// absorbs remaining width like the queue pane's summary, per user request).
const WT_LAST_MIN: usize = 12;

