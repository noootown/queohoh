import type { Exec } from "./resolver-io.js";

export async function discoverItems(
	command: string,
	exec: Exec,
	opts: { cwd: string },
): Promise<Record<string, string>[]> {
	const { stdout, exitCode } = await exec("/bin/bash", ["-lc", command], {
		cwd: opts.cwd,
	});
	if (exitCode !== 0) {
		throw new Error(`discovery command failed (exit ${exitCode})`);
	}
	const parsed: unknown = JSON.parse(stdout.trim());
	if (!Array.isArray(parsed)) {
		throw new Error("discovery command must return a JSON array");
	}
	return parsed.map((raw) => {
		const item: Record<string, string> = {};
		for (const [k, v] of Object.entries(raw as Record<string, unknown>)) {
			item[k] = String(v);
		}
		return item;
	});
}
