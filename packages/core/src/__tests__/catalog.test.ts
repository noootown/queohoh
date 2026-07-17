import { describe, expect, it } from "vitest";
import {
	BUILTIN_CATALOG,
	type CatalogEntry,
	effectiveCatalog,
	findModel,
	formatModel,
	groupHead,
	modelRef,
	PROVIDER_PRECEDENCE,
	unknownModelError,
} from "../catalog.js";

describe("PROVIDER_PRECEDENCE", () => {
	it("is claude, grok, codex in order", () => {
		expect(PROVIDER_PRECEDENCE).toEqual(["claude", "grok", "codex"]);
	});
});

describe("BUILTIN_CATALOG", () => {
	it("is grouped by provider in precedence order, each group most->least powerful", () => {
		expect(BUILTIN_CATALOG).toEqual([
			{ provider: "claude", id: "claude-fable-5", label: "fable" },
			{ provider: "claude", id: "claude-opus-4-8", label: "opus" },
			{ provider: "claude", id: "claude-sonnet-5", label: "sonnet" },
			{ provider: "claude", id: "claude-haiku-4-5", label: "haiku" },
			{ provider: "grok", id: "grok-4.5", label: "grok-4.5" },
			{ provider: "grok", id: "grok-composer-2.5-fast", label: "composer" },
			{ provider: "codex", id: "gpt-5.6-sol", label: "sol" },
			{ provider: "codex", id: "gpt-5.6-terra", label: "terra" },
			{ provider: "codex", id: "gpt-5.6-luna", label: "luna" },
		]);
	});
});

describe("effectiveCatalog", () => {
	it("returns the built-in catalog unchanged when overlay is undefined", () => {
		expect(effectiveCatalog(undefined)).toEqual(BUILTIN_CATALOG);
	});

	it("merges an overlay entry onto an existing built-in without reordering the group", () => {
		const overlay: CatalogEntry[] = [
			{ provider: "claude", id: "claude-opus-4-8", label: "opus-renamed" },
		];
		const result = effectiveCatalog(overlay);
		expect(result).not.toHaveProperty("error");
		const claudeGroup = (result as CatalogEntry[]).filter(
			(e) => e.provider === "claude",
		);
		// Position unchanged (still 2nd in the claude group), only the label field updated.
		expect(claudeGroup.map((e) => e.id)).toEqual([
			"claude-fable-5",
			"claude-opus-4-8",
			"claude-sonnet-5",
			"claude-haiku-4-5",
		]);
		expect(claudeGroup[1]).toEqual({
			provider: "claude",
			id: "claude-opus-4-8",
			label: "opus-renamed",
		});
	});

	it("appends a new overlay entry to the end of its provider's group", () => {
		const overlay: CatalogEntry[] = [
			{ provider: "grok", id: "grok-new-model", label: "grok-new" },
		];
		const result = effectiveCatalog(overlay) as CatalogEntry[];
		const grokGroup = result.filter((e) => e.provider === "grok");
		expect(grokGroup.map((e) => e.label)).toEqual([
			"grok-4.5",
			"composer",
			"grok-new",
		]);
	});

	it("preserves hidden: true set by an overlay entry", () => {
		const overlay: CatalogEntry[] = [
			{
				provider: "claude",
				id: "claude-haiku-4-5",
				label: "haiku",
				hidden: true,
			},
		];
		const result = effectiveCatalog(overlay) as CatalogEntry[];
		const haiku = findModel(result, "claude/haiku");
		expect(haiku?.hidden).toBe(true);
	});

	it("cannot interleave provider groups, regardless of overlay entry order", () => {
		const overlay: CatalogEntry[] = [
			{ provider: "grok", id: "grok-new-model", label: "grok-new" },
			{ provider: "claude", id: "claude-new-model", label: "claude-new" },
			{ provider: "codex", id: "gpt-new-model", label: "codex-new" },
		];
		const result = effectiveCatalog(overlay) as CatalogEntry[];
		const providers = result.map((e) => e.provider);
		// Each provider's entries must be contiguous — no interleaving.
		const firstIndex = new Map<string, number>();
		const lastIndex = new Map<string, number>();
		providers.forEach((p, i) => {
			if (!firstIndex.has(p)) firstIndex.set(p, i);
			lastIndex.set(p, i);
		});
		for (const p of firstIndex.keys()) {
			const first = firstIndex.get(p) as number;
			const last = lastIndex.get(p) as number;
			const span = providers.slice(first, last + 1);
			expect(span.every((x) => x === p)).toBe(true);
		}
		// Groups appear in precedence order: claude, grok, codex.
		expect([...new Set(providers)]).toEqual(["claude", "grok", "codex"]);
	});

	it("puts an overlay entry with an unknown provider into a trailing group after precedence", () => {
		const overlay: CatalogEntry[] = [
			{ provider: "mistral", id: "mistral-large", label: "large" },
		];
		const result = effectiveCatalog(overlay) as CatalogEntry[];
		expect(result.at(-1)).toEqual({
			provider: "mistral",
			id: "mistral-large",
			label: "large",
		});
		const providers = [...new Set(result.map((e) => e.provider))];
		expect(providers).toEqual(["claude", "grok", "codex", "mistral"]);
	});

	it("errors on a duplicate label within one provider", () => {
		const overlay: CatalogEntry[] = [
			{ provider: "claude", id: "claude-new-model", label: "opus" },
		];
		expect(effectiveCatalog(overlay)).toEqual({
			error: "catalog: duplicate label opus in provider claude",
		});
	});
});

describe("findModel", () => {
	it("matches by label within the referenced provider", () => {
		expect(findModel(BUILTIN_CATALOG, "claude/opus")).toEqual({
			provider: "claude",
			id: "claude-opus-4-8",
			label: "opus",
		});
	});

	it("matches by exact id when no label matches", () => {
		expect(findModel(BUILTIN_CATALOG, "claude/claude-opus-4-8")).toEqual({
			provider: "claude",
			id: "claude-opus-4-8",
			label: "opus",
		});
	});

	it("still matches a hidden entry (hidden is picker-only)", () => {
		const catalog: CatalogEntry[] = [
			{
				provider: "claude",
				id: "claude-opus-4-8",
				label: "opus",
				hidden: true,
			},
		];
		expect(findModel(catalog, "claude/opus")).toEqual(catalog[0]);
	});

	it("returns undefined for an unknown ref", () => {
		expect(findModel(BUILTIN_CATALOG, "claude/nonexistent")).toBeUndefined();
	});

	it("does not match across provider groups", () => {
		expect(findModel(BUILTIN_CATALOG, "grok/opus")).toBeUndefined();
	});
});

describe("unknownModelError", () => {
	it("suggests provider/label when the bare ref matches a label in some provider", () => {
		expect(unknownModelError(BUILTIN_CATALOG, "opus")).toBe(
			"unknown model: opus (did you mean claude/opus?)",
		);
	});

	it("suggests provider/label when the part after / matches a label or id", () => {
		expect(unknownModelError(BUILTIN_CATALOG, "grok/opus")).toBe(
			"unknown model: grok/opus (did you mean claude/opus?)",
		);
	});

	it("has no suggestion suffix when nothing matches", () => {
		expect(unknownModelError(BUILTIN_CATALOG, "nonexistent")).toBe(
			"unknown model: nonexistent",
		);
	});
});

describe("groupHead", () => {
	it("returns the first entry of a provider's group", () => {
		expect(groupHead(BUILTIN_CATALOG, "claude")).toEqual({
			provider: "claude",
			id: "claude-fable-5",
			label: "fable",
		});
	});

	it("returns undefined for an unknown provider", () => {
		expect(groupHead(BUILTIN_CATALOG, "mistral")).toBeUndefined();
	});
});

describe("formatModel / modelRef", () => {
	const entry: CatalogEntry = {
		provider: "claude",
		id: "claude-opus-4-8",
		label: "opus",
	};

	it("formatModel renders 'label (provider)'", () => {
		expect(formatModel(entry)).toBe("opus (claude)");
	});

	it("modelRef renders 'provider/label'", () => {
		expect(modelRef(entry)).toBe("claude/opus");
	});
});
