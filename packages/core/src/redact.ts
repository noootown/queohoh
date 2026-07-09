/** Keys whose name looks secret-shaped; only these get collected. */
const SECRET_NAME_RE = /(TOKEN|SECRET|KEY|PASSWORD|PASSWD|CREDENTIAL|AUTH|API)/;

/**
 * Build a Map<secretValue, keyName> from secret-shaped env keys only.
 * A key qualifies when its name matches SECRET_NAME_RE and its value is
 * at least 8 characters — this avoids mangling benign env like SHLVL/LESS.
 */
export function buildSecretMap(
	envEntries: Record<string, string | undefined>,
): Map<string, string> {
	const map = new Map<string, string>();
	for (const [key, value] of Object.entries(envEntries)) {
		if (!value || value.length < 8) continue;
		if (SECRET_NAME_RE.test(key)) {
			map.set(value, key);
		}
	}
	return map;
}

/**
 * Replace secret values with [REDACTED:KEY_NAME]. Longer values replaced first.
 * Also redacts each secret's JSON-escaped form (e.g. embedded quotes/newlines)
 * so secrets survive JSON.stringify without leaking.
 */
export function redact(text: string, secrets: Map<string, string>): string {
	if (secrets.size === 0) return text;
	const forms: Array<[string, string]> = [];
	for (const [value, key] of secrets) {
		forms.push([value, key]);
		const escaped = JSON.stringify(value).slice(1, -1);
		if (escaped !== value) forms.push([escaped, key]);
	}
	forms.sort((a, b) => b[0].length - a[0].length);
	let result = text;
	for (const [form, key] of forms) {
		result = result.replaceAll(form, `[REDACTED:${key}]`);
	}
	return result;
}

export type Redactor = (s: string) => string;

export function makeRedactor(secrets: Map<string, string>): Redactor {
	return (s) => redact(s, secrets);
}
