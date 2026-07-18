/**
 * Model catalog (design spec Section 1). One flat, provider-grouped list of
 * concrete models replaces the old per-provider tier tables: providers
 * appear in fixed precedence order (claude -> grok -> codex), and each
 * provider's entries are ordered most->least powerful within the group.
 *
 * `models.ts` builds per-task chains on top of this; `config.ts` layers a
 * `catalog:` overlay on top of `BUILTIN_CATALOG` via `effectiveCatalog`.
 */

/** One concrete model in the catalog. `id` is the provider-specific model
 * id passed to the CLI; `label` is the reference used in `model:` fields and
 * pickers (`provider/label`). Labels follow `provider-family-version` with
 * hyphens between segments and dots inside the version (e.g.
 * `claude-opus-4.8`, `grok-4.5`) so the version is visible in the TUI without
 * a separate short alias. `hidden` affects pickers only — a hidden entry
 * still resolves when referenced explicitly. */
export interface CatalogEntry {
	provider: string;
	id: string;
	label: string;
	hidden?: boolean;
}

/** Fixed provider group order every catalog is re-grouped into. */
export const PROVIDER_PRECEDENCE: string[] = ["claude", "grok", "codex"];

/** Shipped defaults (design spec Section 1). Grouped by
 * `PROVIDER_PRECEDENCE`; each group ordered most->least powerful.
 * Labels are versioned ids (`provider-family-version`); `id` is what the CLI
 * receives (may still use the provider's native hyphenated form). */
export const BUILTIN_CATALOG: CatalogEntry[] = [
	{ provider: "claude", id: "claude-fable-5", label: "claude-fable-5" },
	{ provider: "claude", id: "claude-opus-4-8", label: "claude-opus-4.8" },
	{ provider: "claude", id: "claude-sonnet-5", label: "claude-sonnet-5" },
	{ provider: "claude", id: "claude-haiku-4-5", label: "claude-haiku-4.5" },
	{ provider: "grok", id: "grok-4.5", label: "grok-4.5" },
	// Hidden from pickers (the grok group offers only grok-4.5) but still
	// resolvable when referenced explicitly — `hidden` is picker-only.
	{
		provider: "grok",
		id: "grok-composer-2.5-fast",
		label: "grok-composer-2.5-fast",
		hidden: true,
	},
	{ provider: "codex", id: "gpt-5.6-sol", label: "gpt-5.6-sol" },
	{ provider: "codex", id: "gpt-5.6-terra", label: "gpt-5.6-terra" },
	{ provider: "codex", id: "gpt-5.6-luna", label: "gpt-5.6-luna" },
];

/**
 * Layer a config `catalog:` overlay onto `BUILTIN_CATALOG`.
 *
 * Merge rules (design spec Section 1):
 * - Overlay entries merge onto built-ins keyed by `provider + "/" + id`:
 *   overlay wins per field (e.g. `hidden`), unmentioned built-ins keep their
 *   position.
 * - Overlay entries with no matching `provider/id` are new — they append at
 *   the END of their provider's group.
 * - An overlay entry naming a provider outside `PROVIDER_PRECEDENCE` starts
 *   a trailing group after the precedence groups.
 * - The result is always re-grouped by provider precedence (then trailing
 *   unknown providers, in first-seen order) so a reorder can never
 *   interleave providers.
 * - A provider group with two entries sharing one `label` is invalid.
 */
export function effectiveCatalog(
	overlay: CatalogEntry[] | undefined,
): CatalogEntry[] | { error: string } {
	const groups = new Map<string, Map<string, CatalogEntry>>();
	const providerOrder: string[] = [...PROVIDER_PRECEDENCE];

	for (const entry of BUILTIN_CATALOG) {
		let group = groups.get(entry.provider);
		if (!group) {
			group = new Map();
			groups.set(entry.provider, group);
		}
		group.set(entry.id, { ...entry });
	}

	for (const overlayEntry of overlay ?? []) {
		let group = groups.get(overlayEntry.provider);
		if (!group) {
			group = new Map();
			groups.set(overlayEntry.provider, group);
			if (!providerOrder.includes(overlayEntry.provider)) {
				providerOrder.push(overlayEntry.provider);
			}
		}
		const existing = group.get(overlayEntry.id);
		group.set(
			overlayEntry.id,
			existing ? { ...existing, ...overlayEntry } : { ...overlayEntry },
		);
	}

	for (const [provider, group] of groups) {
		const seenLabels = new Set<string>();
		for (const entry of group.values()) {
			if (seenLabels.has(entry.label)) {
				return {
					error: `catalog: duplicate label ${entry.label} in provider ${provider}`,
				};
			}
			seenLabels.add(entry.label);
		}
	}

	const result: CatalogEntry[] = [];
	for (const provider of providerOrder) {
		const group = groups.get(provider);
		if (!group) continue;
		for (const entry of group.values()) {
			result.push(entry);
		}
	}
	return result;
}

/** Look up a `provider/label` (or `provider/id` exact-match fallback) ref
 * within its named provider group only. Hidden entries still match — hidden
 * is picker-only.
 *
 * After exact label/id miss, two short-form fallbacks keep pre-versioned
 * refs resolvable so a catalog label rename does not blank the TASKS Model
 * column (or fail every run) for configs still using the older form:
 * 1. `label.endsWith("-" + rest)` — e.g. `claude/sonnet-5` → `claude-sonnet-5`
 *    (intermediate label that gained a provider prefix).
 * 2. pure-alphabetic `rest` matching a hyphen segment of a label — e.g.
 *    `claude/opus` → `claude-opus-4.8`, `claude/sonnet` → `claude-sonnet-5`
 *    (short family tokens from the tier-alias era). Group order is
 *    most→least powerful, so the first match is the current top of that
 *    family. Never crosses provider groups. */
export function findModel(
	catalog: CatalogEntry[],
	ref: string,
): CatalogEntry | undefined {
	const slashIndex = ref.indexOf("/");
	if (slashIndex === -1) return undefined;
	const provider = ref.slice(0, slashIndex);
	const rest = ref.slice(slashIndex + 1);
	const group = catalog.filter((e) => e.provider === provider);
	const exact =
		group.find((e) => e.label === rest) ?? group.find((e) => e.id === rest);
	if (exact !== undefined) return exact;
	// Suffix form: rest is a trailing portion of a versioned label.
	if (/[A-Za-z]/.test(rest)) {
		const bySuffix = group.find((e) => e.label.endsWith(`-${rest}`));
		if (bySuffix !== undefined) return bySuffix;
	}
	// Family-token form: pure alphabetic short name (opus/sonnet/haiku/…).
	if (/^[A-Za-z]+$/.test(rest)) {
		return group.find((e) => e.label.split("-").includes(rest));
	}
	return undefined;
}

/** Build the `unknown model: <ref>` error, with a `did you mean
 * provider/label?` suggestion when the part after `/` (or the whole ref
 * when there is no `/`) matches some entry's label or id in any provider. */
export function unknownModelError(
	catalog: CatalogEntry[],
	ref: string,
): string {
	const slashIndex = ref.indexOf("/");
	const part = slashIndex === -1 ? ref : ref.slice(slashIndex + 1);
	const match = catalog.find((e) => e.label === part || e.id === part);
	const suffix = match ? ` (did you mean ${modelRef(match)}?)` : "";
	return `unknown model: ${ref}${suffix}`;
}

/** First entry of a provider's group ("a provider's most powerful model"),
 * or `undefined` if the provider has no entries in this catalog. */
export function groupHead(
	catalog: CatalogEntry[],
	provider: string,
): CatalogEntry | undefined {
	return catalog.find((e) => e.provider === provider);
}

/** Display form: `label (provider)`. */
export function formatModel(e: CatalogEntry): string {
	return `${e.label} (${e.provider})`;
}

/** Reference form: `provider/label`. */
export function modelRef(e: CatalogEntry): string {
	return `${e.provider}/${e.label}`;
}
