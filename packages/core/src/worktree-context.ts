/**
 * Derive a ticket id from a branch name. The user convention is that a branch
 * is named after its ticket, so the first `LETTERS-DIGITS` token IS the ticket:
 * `JUS-1008` and `jus-1008-fix-thing` both yield `JUS-1008`. Returns the first
 * match uppercased, or "" when the branch carries no ticket-shaped token.
 */
export function extractTicket(branch: string): string {
	const match = branch.match(/[A-Za-z]+-\d+/);
	return match ? match[0].toUpperCase() : "";
}

/**
 * Arg values implied by a worktree context, keyed by the arg-name convention:
 * an arg named `source` or `branch` IS the worktree's branch, `ticket` is the
 * ticket extracted from it. Empty when there is no branch; `ticket` omitted
 * when the branch carries no ticket token. Callers overlay these onto a
 * definition's args form so the user is never asked for what the selected
 * worktree already decides.
 */
export function contextArgValues(
	branch: string | null | undefined,
): Record<string, string> {
	if (!branch) return {};
	const values: Record<string, string> = { source: branch, branch };
	const ticket = extractTicket(branch);
	// Omit the key entirely (rather than set "") so a def with a `ticket` arg
	// and a default falls back to that default instead of a blank override.
	if (ticket !== "") values.ticket = ticket;
	return values;
}
