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
	};
	const leave = () => {
		if (!entered) return;
		entered = false;
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
