import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

export interface ClaudeSessionInfo {
	sessionId: string;
	mtimeMs: number;
	aiTitle: string | null;
	firstPrompt: string | null;
}

/** Claude Code's project-dir encoding: the absolute cwd with every `/` and
 * `.` replaced by `-` (verified against ~/.claude/projects on disk). */
export function encodeProjectDir(absPath: string): string {
	return absPath.replace(/[/.]/g, "-");
}

export function listClaudeSessions(
	claudeProjectsDir: string,
	worktreePath: string,
	limit = 5,
): ClaudeSessionInfo[] {
	const dir = join(claudeProjectsDir, encodeProjectDir(worktreePath));
	let names: string[];
	try {
		names = readdirSync(dir);
	} catch {
		return [];
	}
	const files: { path: string; sessionId: string; mtimeMs: number }[] = [];
	for (const name of names) {
		if (!name.endsWith(".jsonl")) continue; // subdirs hold subagent transcripts
		const path = join(dir, name);
		try {
			const st = statSync(path);
			if (!st.isFile()) continue;
			files.push({
				path,
				sessionId: name.slice(0, -".jsonl".length),
				mtimeMs: st.mtimeMs,
			});
		} catch {
			// raced deletion — skip
		}
	}
	files.sort((a, b) => b.mtimeMs - a.mtimeMs);
	return files.slice(0, limit).map((f) => {
		const { aiTitle, firstPrompt } = extractLabels(f.path);
		return { sessionId: f.sessionId, mtimeMs: f.mtimeMs, aiTitle, firstPrompt };
	});
}

function extractLabels(path: string): {
	aiTitle: string | null;
	firstPrompt: string | null;
} {
	let aiTitle: string | null = null;
	let firstPrompt: string | null = null;
	let text: string;
	try {
		text = readFileSync(path, "utf-8");
	} catch {
		return { aiTitle, firstPrompt };
	}
	for (const line of text.split("\n")) {
		if (line === "") continue;
		// Cheap substring pre-filters keep JSON.parse off bulky records.
		const maybeTitle = line.includes('"ai-title"');
		const maybePrompt = firstPrompt === null && line.includes('"user"');
		if (!maybeTitle && !maybePrompt) continue;
		let record: unknown;
		try {
			record = JSON.parse(line);
		} catch {
			continue;
		}
		if (record === null || typeof record !== "object") continue;
		const r = record as Record<string, unknown>;
		if (
			r.type === "ai-title" &&
			typeof r.aiTitle === "string" &&
			r.aiTitle !== ""
		) {
			aiTitle = r.aiTitle; // last one wins — titles refresh as the session evolves
		}
		if (firstPrompt === null && r.type === "user") {
			const content = (r.message as Record<string, unknown> | undefined)
				?.content;
			let textContent: string | null = null;
			if (typeof content === "string") textContent = content;
			else if (Array.isArray(content)) {
				const block = content.find(
					(c) =>
						c !== null &&
						typeof c === "object" &&
						(c as Record<string, unknown>).type === "text",
				) as Record<string, unknown> | undefined;
				if (block && typeof block.text === "string") textContent = block.text;
			}
			if (textContent !== null) {
				const firstLine = (textContent.split("\n", 1)[0] ?? "").trim();
				if (firstLine !== "") firstPrompt = firstLine.slice(0, 120);
			}
		}
	}
	return { aiTitle, firstPrompt };
}
