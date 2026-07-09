/** XML-escape a string for safe interpolation into plist text. */
function esc(s: string): string {
	return s
		.replaceAll("&", "&amp;")
		.replaceAll("<", "&lt;")
		.replaceAll(">", "&gt;");
}

export function launchdPlist(opts: {
	label: string;
	nodeBin: string;
	cliPath: string;
	logPath: string;
}): string {
	return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>${esc(opts.label)}</string>
	<key>ProgramArguments</key>
	<array>
		<string>${esc(opts.nodeBin)}</string>
		<string>${esc(opts.cliPath)}</string>
		<string>daemon</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<true/>
	<key>StandardOutPath</key>
	<string>${esc(opts.logPath)}</string>
	<key>StandardErrorPath</key>
	<string>${esc(opts.logPath)}</string>
</dict>
</plist>
`;
}
