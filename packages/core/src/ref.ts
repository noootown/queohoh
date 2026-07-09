export type TargetRef =
	| { kind: "pr"; number: number }
	| { kind: "ticket"; id: string }
	| { kind: "worktree"; name: string }
	| { kind: "temp" };

const TICKET_RE = /([A-Z][A-Z0-9]*-\d+)/;
const TICKET_FULL_RE = /^[A-Z][A-Z0-9]*-\d+$/;

export function parseRef(raw: string): TargetRef {
	if (raw === "temp") return { kind: "temp" };
	const [kind, ...rest] = raw.split(":");
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
	}
}

export function extractTicketId(text: string): string | null {
	return TICKET_RE.exec(text)?.[1] ?? null;
}
