export type TargetRef =
	| { kind: "pr"; number: number }
	| { kind: "ticket"; id: string }
	| { kind: "worktree"; name: string }
	| { kind: "temp" }
	| { kind: "repo" };

const TICKET_RE = /([A-Z][A-Z0-9]*-\d+)/;
const TICKET_FULL_RE = /^[A-Z][A-Z0-9]*-\d+$/;

// Paste-friendly URL forms. Scheme is optional so a bare `github.com/...` paste
// still resolves; a trailing segment/query/fragment (e.g. `/files`, `?diff=split`,
// `#discussion_r1`) is tolerated and ignored.
const GITHUB_PR_RE =
	/^(?:https?:\/\/)?github\.com\/[^/\s]+\/[^/\s]+\/pull\/(\d+)(?:[/?#].*)?$/;
const LINEAR_ISSUE_RE =
	/^(?:https?:\/\/)?linear\.app\/[^/\s]+\/issue\/([^/?#\s]+)/;

// Non-anchored twins of the URL patterns above, for scanning freeform prose
// where the URL sits somewhere inside the text rather than being the whole
// string. `.exec` (no global flag) returns the first match, left to right.
const GITHUB_PR_IN_TEXT_RE =
	/(?:https?:\/\/)?github\.com\/[^/\s]+\/[^/\s]+\/pull\/(\d+)/;
const LINEAR_ISSUE_IN_TEXT_RE =
	/(?:https?:\/\/)?linear\.app\/[^/\s]+\/issue\/([^/?#\s]+)/;

export function parseRef(raw: string): TargetRef {
	const trimmed = raw.trim();
	if (trimmed === "temp") return { kind: "temp" };
	if (trimmed === "repo") return { kind: "repo" };
	const [kind, ...rest] = trimmed.split(":");
	const value = rest.join(":");
	if (kind === "pr" && /^\d+$/.test(value)) {
		return { kind: "pr", number: Number(value) };
	}
	if (kind === "ticket" && TICKET_FULL_RE.test(value)) {
		return { kind: "ticket", id: value };
	}
	if (kind === "worktree" && value.length > 0) {
		return { kind: "worktree", name: value };
	}
	// Bare ticket id, `#N`, and PR/issue URLs — pasted straight from a browser or
	// tracker. Bare numbers stay rejected: `123` is ambiguous between pr and ticket.
	if (TICKET_FULL_RE.test(trimmed)) {
		return { kind: "ticket", id: trimmed };
	}
	const hash = /^#(\d+)$/.exec(trimmed);
	if (hash) {
		return { kind: "pr", number: Number(hash[1]) };
	}
	const prUrl = GITHUB_PR_RE.exec(trimmed);
	if (prUrl) {
		return { kind: "pr", number: Number(prUrl[1]) };
	}
	const linearIssue = LINEAR_ISSUE_RE.exec(trimmed);
	if (linearIssue) {
		const id = extractTicketId(linearIssue[1] ?? "");
		if (id) return { kind: "ticket", id };
	}
	throw new Error(`invalid ref: ${raw}`);
}

export function formatRef(ref: TargetRef): string {
	switch (ref.kind) {
		case "pr":
			return `pr:${ref.number}`;
		case "ticket":
			return `ticket:${ref.id}`;
		case "worktree":
			return `worktree:${ref.name}`;
		case "temp":
			return "temp";
		case "repo":
			return "repo";
	}
}

export function extractTicketId(text: string): string | null {
	return TICKET_RE.exec(text)?.[1] ?? null;
}

/**
 * Best-effort ref extraction from freeform prose (e.g. a task's `situation`
 * arg). Precedence: the first GitHub PR URL anywhere in the text wins; failing
 * that, the first Linear issue URL whose slug carries a ticket id; failing that,
 * a ticket id only if it is the *leading* token of the text.
 *
 * The leading-token rule is deliberately narrow: prose is never scanned for
 * bare ticket ids, because tokens like `SHA-256`, `UTF-8`, and `HTTP-2` share
 * the ticket shape and would extract spuriously mid-sentence.
 */
export function extractRef(text: string): TargetRef | null {
	const prUrl = GITHUB_PR_IN_TEXT_RE.exec(text);
	if (prUrl) return { kind: "pr", number: Number(prUrl[1]) };

	const linearIssue = LINEAR_ISSUE_IN_TEXT_RE.exec(text);
	if (linearIssue) {
		const id = extractTicketId(linearIssue[1] ?? "");
		if (id) return { kind: "ticket", id };
	}

	// Leading token only — strip trailing punctuation (`JUS-1821:` from a
	// sentence lead-in) before testing for a full ticket id.
	const leadToken = text.trim().split(/\s+/)[0] ?? "";
	const firstToken = leadToken.replace(/[.,:]+$/, "");
	if (TICKET_FULL_RE.test(firstToken)) {
		return { kind: "ticket", id: firstToken };
	}

	return null;
}
