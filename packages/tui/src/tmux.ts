import { execFile } from "node:child_process";

/** True when running inside a tmux client (the `TMUX` env var is set). */
export function insideTmux(env: NodeJS.ProcessEnv = process.env): boolean {
	return typeof env.TMUX === "string" && env.TMUX.length > 0;
}

/**
 * Open a new tmux window with its cwd set to `path` — the TUI's stand-in for
 * "cd to the worktree" (a child process cannot change the parent shell's cwd).
 * Resolves to an error string for the status line, or null on success.
 */
export function openTmuxWindow(path: string): Promise<string | null> {
	return new Promise((resolve) => {
		execFile("tmux", ["new-window", "-c", path], (error) => {
			resolve(error ? `tmux new-window failed: ${error.message}` : null);
		});
	});
}
