/**
 * Escape a value for a systemd unit-file assignment.
 *
 * Unit values may be double-quoted; inside quotes, `\`, `"`, and newlines must
 * be backslash-escaped. `%` is doubled so systemd does not treat it as a
 * specifier. Unquoted paths are fine for typical absolute node/cli/log paths
 * (no spaces), so we only quote when the value contains whitespace or a
 * character that would otherwise need quoting.
 *
 * See systemd.syntax(7) and systemd.unit(5).
 */
export function escapeSystemdValue(s: string): string {
	// Specifiers: any lone `%` becomes `%%` so a path like `%i` is literal.
	const percentSafe = s.replaceAll("%", "%%");
	const needsQuotes = /[\s"'\\]/.test(percentSafe) || percentSafe.length === 0;
	if (!needsQuotes) return percentSafe;
	const escaped = percentSafe
		.replaceAll("\\", "\\\\")
		.replaceAll('"', '\\"')
		.replaceAll("\n", "\\n");
	return `"${escaped}"`;
}

/**
 * Render a systemd user unit that keeps the daemon alive across exits.
 *
 * `KillMode=process` is load-bearing: the default (`control-group`) would kill
 * every process in the service cgroup on stop/restart, including detached run
 * shims that are supposed to survive a daemon reload. Only the main daemon
 * process is signalled; shims re-adopt on the next start.
 */
export function systemdUnit(opts: {
	nodeBin: string;
	cliPath: string;
	logPath: string;
}): string {
	const nodeBin = escapeSystemdValue(opts.nodeBin);
	const cliPath = escapeSystemdValue(opts.cliPath);
	const logPath = escapeSystemdValue(opts.logPath);
	return `[Unit]
Description=queohoh task-queue daemon

[Service]
Type=simple
ExecStart=${nodeBin} ${cliPath} daemon
Restart=always
RestartSec=2
KillMode=process
StandardOutput=append:${logPath}
StandardError=append:${logPath}

[Install]
WantedBy=default.target
`;
}
