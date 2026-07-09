import type { Exec } from "./resolver-io.js";

export async function execHook(
	cmd: string,
	exec: Exec,
	opts: { cwd: string },
): Promise<void> {
	const { exitCode } = await exec("/bin/bash", ["-lc", cmd], { cwd: opts.cwd });
	if (exitCode !== 0) {
		throw new Error(`hook failed (exit ${exitCode}): ${cmd}`);
	}
}
