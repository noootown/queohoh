export function render(
	template: string,
	globalVars: Record<string, string> = {},
	repoVars: Record<string, string> = {},
	itemVars: Record<string, string> = {},
	reservedVars: Record<string, string> = {},
): string {
	const merged = { ...globalVars, ...repoVars, ...itemVars, ...reservedVars };
	return template.replace(/\{\{(\w+)\}\}/g, (match, key) =>
		key in merged ? String(merged[key]) : match,
	);
}
