import { EventEmitter } from "node:events";
import { Text } from "ink";
import { render } from "ink-testing-library";
import { afterEach, describe, expect, it } from "vitest";
import { useTerminalSize } from "../use-terminal-size.js";

type FakeStream = EventEmitter & { columns: number; rows: number };

function fakeStream(columns: number, rows: number): FakeStream {
	const emitter = new EventEmitter() as FakeStream;
	emitter.columns = columns;
	emitter.rows = rows;
	return emitter;
}

const cleanups: Array<() => void> = [];
afterEach(() => {
	while (cleanups.length) cleanups.pop()?.();
});

function Probe({ stream }: { stream: FakeStream }) {
	const { columns, rows } = useTerminalSize(
		stream as unknown as NodeJS.WriteStream,
	);
	return <Text>{`${columns}x${rows}`}</Text>;
}

const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

describe("useTerminalSize", () => {
	it("renders the stream size and re-renders on resize", async () => {
		const stream = fakeStream(100, 40);
		const app = render(<Probe stream={stream} />);
		cleanups.push(() => app.unmount());
		expect(app.lastFrame()).toBe("100x40");
		stream.columns = 120;
		stream.rows = 50;
		stream.emit("resize");
		await wait(0);
		expect(app.lastFrame()).toBe("120x50");
	});
});
