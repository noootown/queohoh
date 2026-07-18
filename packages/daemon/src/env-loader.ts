import { readFileSync } from "node:fs";
import { join } from "node:path";
import { parseEnvFile } from "@queohoh/core";

/**
 * Load `<workspacePath>/.env` into `env` (default `process.env`) at daemon
 * startup so run secrets kept in the workspace reach spawned runs — the daemon
 * otherwise injects only its own environment (see runner.ts), and under launchd
 * that environment is minimal. Must run BEFORE `buildSecretMap(process.env)` so
 * secret-shaped keys are redacted like any other env secret.
 *
 * Precedence: a key already present in `env` wins (the file only fills gaps), so
 * an operator can still override a single secret via launchd/shell. A missing
 * file (or any read/parse error) is a no-op. Returns the keys it set, for
 * caller logging.
 */
export function loadWorkspaceEnv(
	workspacePath: string,
	env: NodeJS.ProcessEnv = process.env,
): string[] {
	const path = join(workspacePath, ".env");
	try {
		const text = readFileSync(path, "utf-8");
		const set: string[] = [];
		for (const [key, value] of Object.entries(parseEnvFile(text))) {
			if (env[key] !== undefined) continue;
			env[key] = value;
			set.push(key);
		}
		return set;
	} catch (err) {
		if ((err as NodeJS.ErrnoException).code !== "ENOENT") {
			console.warn(
				`could not read ${path}: ${err instanceof Error ? err.message : String(err)}`,
			);
		}
		return [];
	}
}
