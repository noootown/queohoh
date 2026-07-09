/**
 * Validate a branch name for the create-worktree modal, returning an error
 * message when it is not git-ref-safe or `null` when it is acceptable. Checks run
 * in the order the message should surface: non-empty, no whitespace, no `..`, no
 * leading `-`/`/`, no trailing `.lock`, printable ASCII only.
 */
export function validateBranchName(name: string): string | null {
	if (name.length === 0) return "branch name required";
	if (/\s/.test(name)) return "no whitespace allowed";
	if (name.includes("..")) return "no '..' allowed";
	if (name.startsWith("-") || name.startsWith("/")) {
		return "cannot start with '-' or '/'";
	}
	if (name.endsWith(".lock")) return "cannot end with '.lock'";
	if (/[^\x20-\x7e]/.test(name)) return "printable ASCII only";
	return null;
}
