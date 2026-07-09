/**
 * Lightweight, line-based markdown styler for the DETAIL pane, mirroring
 * agent247's TUI visual language (bold headings, blue links, distinct inline
 * code). It is deliberately regex-per-line/span — no markdown AST — and emits
 * plain style segments that the renderer maps to ink `<Text>` colors. Colors
 * are ink named colors (not raw ANSI) so ink measures/truncates correctly.
 */

export interface Segment {
	text: string;
	color?: string;
	bold?: boolean;
	dim?: boolean;
}

const HEADING = /^#{1,3}\s+(.*)$/;
const RULE = /^---+$/;
// A single line's inline spans, in precedence order: **bold**, `code`, URLs.
const INLINE = /(\*\*[^*]+\*\*)|(`[^`]+`)|(https?:\/\/[^\s)>\]"']+)/g;

/**
 * Split a single line into styled segments. Whole-line rules (headings, rules)
 * win; otherwise the line is tokenized into bold / inline-code / URL spans with
 * the surrounding text left plain. Always returns at least one segment.
 */
export function styleLine(line: string): Segment[] {
	const heading = HEADING.exec(line);
	if (heading) return [{ text: heading[1] ?? "", bold: true }];
	if (RULE.test(line)) return [{ text: line, dim: true }];

	const segments: Segment[] = [];
	let last = 0;
	for (const match of line.matchAll(INLINE)) {
		const index = match.index ?? 0;
		if (index > last) segments.push({ text: line.slice(last, index) });
		const [token, boldTok, codeTok, urlTok] = match;
		if (boldTok) {
			segments.push({ text: boldTok.slice(2, -2), bold: true });
		} else if (codeTok) {
			segments.push({ text: codeTok.slice(1, -1), color: "cyan" });
		} else if (urlTok) {
			segments.push({ text: urlTok, color: "blue" });
		}
		last = index + token.length;
	}
	if (last < line.length) segments.push({ text: line.slice(last) });
	if (segments.length === 0) segments.push({ text: line });
	return segments;
}
