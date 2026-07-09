import { ulid } from "ulid";

const MAX_SLUG_LENGTH = 24;

/**
 * Lowercase `text`, collapse runs of non-alphanumeric characters into a single
 * dash, trim leading/trailing dashes, and truncate to 24 characters preferring a
 * whole-word boundary. Returns an empty string when nothing usable remains.
 */
export function slugify(text: string): string {
	const base = text
		.toLowerCase()
		.replace(/[^a-z0-9]+/g, "-")
		.replace(/^-+|-+$/g, "");
	if (base.length <= MAX_SLUG_LENGTH) return base;
	const truncated = base.slice(0, MAX_SLUG_LENGTH);
	const lastDash = truncated.lastIndexOf("-");
	// Cut at the last word boundary, but only when one sits past the halfway
	// mark — otherwise a very long first word would collapse to nothing.
	const cut =
		lastDash > MAX_SLUG_LENGTH / 2 ? truncated.slice(0, lastDash) : truncated;
	return cut.replace(/-+$/g, "");
}

function ulidSuffix(length: number): string {
	return ulid().slice(-length).toLowerCase();
}

/**
 * Ephemeral worktree name for a temp ref: `qoo-<slug>-<suffix>` derived from the
 * task prompt, with a 4-char suffix for uniqueness across similar prompts. Falls
 * back to `qoo-<ulid6>` when the prompt yields no usable slug.
 */
export function qooTempName(prompt: string): string {
	const slug = slugify(prompt);
	if (slug === "") return `qoo-${ulidSuffix(6)}`;
	return `qoo-${slug}-${ulidSuffix(4)}`;
}
