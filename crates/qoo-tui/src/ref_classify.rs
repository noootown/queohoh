//! Classify a human-typed worktree/target into a canonical ref, or `None` when
//! the input is a literal worktree name. Mirrors `packages/core/src/ref.ts`'s
//! `parseRef` for the cases a person types into the combobox: a bare number or
//! `#N` or a GitHub PR URL → `pr:N`; a full ticket id or a Linear issue URL →
//! `ticket:ID`; an already-canonical `pr:N` / `ticket:ID` / `worktree:NAME` →
//! itself (normalized); anything else → `None` (the caller treats it as a
//! worktree name). Hand-rolled scans (no regex dep, matching
//! `worktree_context`'s style).

/// Canonical ref for a human-typed target, else `None` (treat as a worktree
/// name). See the module doc for the recognized shapes.
pub fn classify_ref(raw: &str) -> Option<String> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    // Already-canonical prefixed refs pass through — this is what the combobox's
    // own synthetic popup row emits (`pr:N`/`ticket:ID`), so picking it and then
    // submitting (which classifies a SECOND time in `resolve_target_ref`) must be
    // idempotent. Without this the colon matches no pattern below → `None` → the
    // caller wraps it as `worktree:pr:N`, which the daemon can't resolve and the
    // task parks needs-input. `parseRef` already accepts these prefixes, so the
    // daemon side handles `pr:N` fine; only this classifier was missing them.
    // A malformed suffix (`pr:abc`, `ticket:foo`, empty `worktree:`) is NOT a
    // ref — let it fall through to `None` / worktree-name semantics.
    if let Some(n) = t.strip_prefix("pr:")
        && !n.is_empty()
        && n.bytes().all(|b| b.is_ascii_digit())
    {
        return Some(format!("pr:{n}"));
    }
    if let Some(id) = t.strip_prefix("ticket:")
        && is_full_ticket(id)
    {
        return Some(format!("ticket:{}", id.to_ascii_uppercase()));
    }
    if let Some(name) = t.strip_prefix("worktree:")
        && !name.is_empty()
    {
        return Some(format!("worktree:{name}"));
    }
    // `#N` or bare `N` → PR.
    let digits = t.strip_prefix('#').unwrap_or(t);
    if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
        return Some(format!("pr:{digits}"));
    }
    // GitHub PR URL → pr:N (…/pull/<N>).
    if let Some(n) = github_pr_number(t) {
        return Some(format!("pr:{n}"));
    }
    // Linear issue URL → ticket:<id> (first LETTERS-DIGITS token in the slug).
    if t.contains("linear.app/")
        && let Some(id) = crate::worktree_context::extract_ticket(t)
    {
        return Some(format!("ticket:{id}"));
    }
    // Whole-string ticket id → ticket.
    if is_full_ticket(t) {
        return Some(format!("ticket:{}", t.to_ascii_uppercase()));
    }
    None
}

/// The PR number in a GitHub PR URL: find `/pull/` and read the trailing digit
/// run, `None` when the segment after `/pull/` is not one-or-more digits.
fn github_pr_number(s: &str) -> Option<String> {
    let idx = s.find("/pull/")?;
    let rest = &s[idx + "/pull/".len()..];
    let digits: String = rest.bytes().take_while(u8::is_ascii_digit).map(|b| b as char).collect();
    if digits.is_empty() { None } else { Some(digits) }
}

/// Whether the WHOLE string is a ticket id `^[A-Za-z][A-Za-z0-9]*-\d+$`: a
/// leading letter, then letters/digits, a single `-`, then one-or-more digits
/// to the end.
fn is_full_ticket(s: &str) -> bool {
    let b = s.as_bytes();
    let n = b.len();
    if n == 0 || !b[0].is_ascii_alphabetic() {
        return false;
    }
    let mut i = 1;
    while i < n && b[i].is_ascii_alphanumeric() {
        i += 1;
    }
    if i >= n || b[i] != b'-' {
        return false;
    }
    i += 1; // consume '-'
    if i >= n {
        return false; // need at least one digit
    }
    b[i..].iter().all(u8::is_ascii_digit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_ref_maps_typed_targets() {
        assert_eq!(classify_ref("45").as_deref(), Some("pr:45"));
        assert_eq!(classify_ref("#45").as_deref(), Some("pr:45"));
        assert_eq!(
            classify_ref("https://github.com/o/r/pull/45").as_deref(),
            Some("pr:45"),
        );
        assert_eq!(classify_ref("JUS-1756").as_deref(), Some("ticket:JUS-1756"));
        assert_eq!(classify_ref("feature-x").as_deref(), None); // literal worktree name
        assert_eq!(classify_ref("").as_deref(), None);
    }

    #[test]
    fn classify_ref_passes_through_canonical_refs() {
        // Already-canonical forms (what the combobox popup emits) round-trip so a
        // second classify pass on submit is idempotent, not `worktree:pr:N`.
        assert_eq!(classify_ref("pr:1925").as_deref(), Some("pr:1925"));
        assert_eq!(classify_ref("ticket:jus-1756").as_deref(), Some("ticket:JUS-1756"));
        assert_eq!(classify_ref("ticket:JUS-1756").as_deref(), Some("ticket:JUS-1756"));
        assert_eq!(classify_ref("worktree:my-branch").as_deref(), Some("worktree:my-branch"));
        // Malformed suffixes are NOT refs — fall through to worktree-name (None).
        assert_eq!(classify_ref("pr:abc").as_deref(), None);
        assert_eq!(classify_ref("ticket:foo").as_deref(), None);
        assert_eq!(classify_ref("worktree:").as_deref(), None);
    }
}
