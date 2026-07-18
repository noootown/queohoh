// packages/core/src/env-file.ts

/**
 * Parse a `.env`-style file into a key→value map. Deliberately small — plain
 * KEY=VALUE secrets only. No variable interpolation and no multiline values.
 * Never throws; malformed lines are skipped so one bad line can't hide the
 * rest.
 *
 * - blank lines and `#` comment lines are ignored
 * - a leading `export ` is stripped
 * - the key must match [A-Za-z_][A-Za-z0-9_]*
 * - surrounding single/double quotes on the value are stripped; an unquoted
 *   value is taken verbatim with trailing whitespace trimmed
 */
export function parseEnvFile(text: string): Record<string, string> {
	const out: Record<string, string> = {};
	for (const rawLine of text.split(/\r?\n/)) {
		let line = rawLine.trim();
		if (line === "" || line.startsWith("#")) continue;
		if (line.startsWith("export ")) line = line.slice("export ".length).trim();
		const eq = line.indexOf("=");
		if (eq === -1) continue;
		const key = line.slice(0, eq).trim();
		if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) continue;
		let value = line.slice(eq + 1);
		const trimmed = value.trim();
		if (
			trimmed.length >= 2 &&
			((trimmed.startsWith('"') && trimmed.endsWith('"')) ||
				(trimmed.startsWith("'") && trimmed.endsWith("'")))
		) {
			value = trimmed.slice(1, -1);
		} else {
			value = value.replace(/\s+$/, "");
		}
		out[key] = value;
	}
	return out;
}
