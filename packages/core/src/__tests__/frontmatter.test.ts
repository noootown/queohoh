import { describe, expect, it } from "vitest";
import { parseFrontmatter, stringifyFrontmatter } from "../frontmatter.js";

describe("parseFrontmatter", () => {
	it("splits meta and body", () => {
		const { meta, body } = parseFrontmatter(
			"---\nid: abc\nnested:\n  a: 1\n---\n\nDo the thing.\n",
		);
		expect(meta).toEqual({ id: "abc", nested: { a: 1 } });
		expect(body).toBe("Do the thing.\n");
	});

	it("keeps --- inside the body", () => {
		const { body } = parseFrontmatter("---\nid: x\n---\n\na\n---\nb\n");
		expect(body).toBe("a\n---\nb\n");
	});

	it("throws on missing frontmatter", () => {
		expect(() => parseFrontmatter("no frontmatter")).toThrow(
			"missing frontmatter",
		);
	});
});

describe("stringifyFrontmatter", () => {
	it("round-trips", () => {
		const meta = { id: "01ABC", n: 5, arr: ["x"] };
		const body = "Prompt text.\n\n## Attachments\nnone\n";
		const out = stringifyFrontmatter(meta, body);
		const back = parseFrontmatter(out);
		expect(back.meta).toEqual(meta);
		expect(back.body).toBe(body);
	});
});
