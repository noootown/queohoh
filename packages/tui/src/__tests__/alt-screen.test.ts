import { describe, expect, it } from "vitest";
import { createAltScreen } from "../alt-screen.js";

function fakeOut(): {
	writes: string[];
	stream: { write: (s: string) => boolean };
} {
	const writes: string[] = [];
	return {
		writes,
		stream: {
			write: (s: string) => {
				writes.push(s);
				return true;
			},
		},
	};
}

describe("alt screen", () => {
	it("enter writes 1049h, leave writes 1049l once", () => {
		const { writes, stream } = fakeOut();
		const alt = createAltScreen(stream as unknown as NodeJS.WriteStream);
		alt.enter();
		alt.leave();
		alt.leave(); // idempotent
		expect(writes).toEqual(["\x1b[?1049h", "\x1b[?1049l"]);
	});

	it("enter is idempotent — only writes 1049h once", () => {
		const { writes, stream } = fakeOut();
		const alt = createAltScreen(stream as unknown as NodeJS.WriteStream);
		alt.enter();
		alt.enter();
		expect(writes).toEqual(["\x1b[?1049h"]);
	});
});
