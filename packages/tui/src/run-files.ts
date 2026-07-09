import {
	closeSync,
	existsSync,
	openSync,
	readFileSync,
	readSync,
	statSync,
} from "node:fs";
import { join } from "node:path";

const TRANSCRIPT_TAIL_LINES = 25;

function readTranscriptTail(
	path: string,
	tailLines: number = TRANSCRIPT_TAIL_LINES,
): string[] {
	// Clamp at the source: `.slice(-tailLines)` with tailLines 0 is `slice(-0)`
	// === `slice(0)`, which would return the whole file instead of one line.
	tailLines = Math.max(1, tailLines);
	const size = statSync(path).size;
	if (size === 0) return [];
	const window = Math.min(262144, Math.max(65536, tailLines * 512));
	const start = Math.max(0, size - window);
	const length = size - start;
	const buffer = Buffer.allocUnsafe(length);
	const fd = openSync(path, "r");
	try {
		let read = 0;
		while (read < length) {
			const n = readSync(fd, buffer, read, length - read, start + read);
			if (n === 0) break;
			read += n;
		}
		return buffer
			.subarray(0, read)
			.toString("utf-8")
			.split("\n")
			.slice(-tailLines);
	} finally {
		closeSync(fd);
	}
}

export function readRunFiles(
	runsDir: string,
	taskId: string,
	opts?: { tailLines?: number },
): { report: string | null; transcriptTail: string[] } {
	const tailLines = opts?.tailLines ?? TRANSCRIPT_TAIL_LINES;
	const dir = join(runsDir, taskId);
	const reportPath = join(dir, "report.md");
	const transcriptPath = join(dir, "transcript.md");
	const report = existsSync(reportPath)
		? readFileSync(reportPath, "utf-8")
		: null;
	const transcriptTail = existsSync(transcriptPath)
		? readTranscriptTail(transcriptPath, tailLines)
		: [];
	return { report, transcriptTail };
}
