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
