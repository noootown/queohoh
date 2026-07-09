import yaml from "js-yaml";

const DELIM = "---\n";

export function parseFrontmatter(content: string): {
	meta: Record<string, unknown>;
	body: string;
} {
	if (!content.startsWith(DELIM)) throw new Error("missing frontmatter");
	const end = content.indexOf(`\n${DELIM}`, DELIM.length);
	if (end === -1) throw new Error("missing frontmatter");
	const rawMeta = content.slice(DELIM.length, end + 1);
	const meta = yaml.load(rawMeta) as Record<string, unknown>;
	if (meta === null || typeof meta !== "object" || Array.isArray(meta)) {
		throw new Error("frontmatter is not a mapping");
	}
	// skip the closing delimiter and at most one blank separator line
	let body = content.slice(end + 1 + DELIM.length);
	if (body.startsWith("\n")) body = body.slice(1);
	return { meta, body };
}

export function stringifyFrontmatter(
	meta: Record<string, unknown>,
	body: string,
): string {
	return `${DELIM}${yaml.dump(meta)}${DELIM}\n${body}`;
}
