/**
 * ANSI escape sequences: CSI (`ESC [ … final-byte`), OSC (`ESC ] … BEL|ST`),
 * and two-byte `ESC x` escapes. Matches what terminals consume as zero-width —
 * exactly the bytes that garble a cell-based renderer, which drops the ESC but
 * prints the printable tail (`[2m`) literally.
 */
const ANSI_RE =
	// biome-ignore lint/suspicious/noControlCharactersInRegex: matching escape bytes is the point
	/\x1b(?:\[[0-9;:?]*[ -/]*[@-~]|\][^\x07\x1b]*(?:\x07|\x1b\\)?|[@-Z\\-_])/g;

/**
 * Clean captured command output (test runners, hooks) for storage and display:
 * strip ANSI escape sequences, then resolve carriage-return overwrites the way
 * a terminal would — each line keeps only its final `\r` segment, so spinner /
 * progress redraws collapse to their last state instead of interleaving.
 */
export function cleanCapturedOutput(text: string): string {
	return text
		.replace(ANSI_RE, "")
		.split("\n")
		.map((line) => {
			const noCrlf = line.endsWith("\r") ? line.slice(0, -1) : line;
			return noCrlf.split("\r").pop() ?? noCrlf;
		})
		.join("\n");
}
