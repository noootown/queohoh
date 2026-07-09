export interface AltScreen {
	enter(): void;
	leave(): void;
	installGuards(): void;
}

export function createAltScreen(
	out: NodeJS.WriteStream = process.stdout,
): AltScreen {
	let entered = false;
	const enter = () => {
		if (entered) return;
		entered = true;
		out.write("\x1b[?1049h");
		// Enable mouse button tracking (1000) in SGR extended mode (1006) so the
		// wheel arrives as `ESC [ < btn ; col ; row M`. This hijacks native text
		// selection (hold Shift/Option to select) — the deliberate Claude-Code
		// tradeoff for scroll-with-the-wheel.
		out.write("\x1b[?1000h\x1b[?1006h");
	};
	const leave = () => {
		if (!entered) return;
		entered = false;
		// Disable mouse tracking before leaving the alt screen so the user's
		// terminal is never left in mouse-reporting mode on exit/crash.
		out.write("\x1b[?1000l\x1b[?1006l");
		out.write("\x1b[?1049l");
	};
	const installGuards = () => {
		process.on("exit", leave);
		process.on("SIGINT", () => {
			leave();
			process.exit(130);
		});
		process.on("SIGTERM", () => {
			leave();
			process.exit(143);
		});
	};
	return { enter, leave, installGuards };
}
