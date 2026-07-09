import { useEffect, useState } from "react";

export interface TerminalSize {
	columns: number;
	rows: number;
}

type SizeStream = Pick<NodeJS.WriteStream, "columns" | "rows"> &
	Pick<NodeJS.EventEmitter, "on" | "off">;

export function useTerminalSize(
	stream: SizeStream = process.stdout,
): TerminalSize {
	const read = () => ({
		columns: stream.columns ?? 80,
		rows: stream.rows ?? 24,
	});
	const [size, setSize] = useState<TerminalSize>(read);
	// biome-ignore lint/correctness/useExhaustiveDependencies: read closes over stream
	useEffect(() => {
		const onResize = () => setSize(read());
		stream.on("resize", onResize);
		return () => {
			stream.off("resize", onResize);
		};
	}, [stream]);
	return size;
}
