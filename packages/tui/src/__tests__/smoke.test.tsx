import { EventEmitter } from "node:events";
import { render } from "ink-testing-library";
import { afterEach, describe, expect, it } from "vitest";
import { App } from "../App.js";
import { createActions } from "../actions.js";
import { cleanups, startServer } from "./helpers.js";

afterEach(async () => {
	while (cleanups.length) await cleanups.pop()?.();
});

const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

// Fixed terminal size so the full-screen layout does not depend on the ambient
// terminal (App pins the root Box to the reported terminal size).
function bigStream(): NodeJS.WriteStream {
	const emitter = new EventEmitter() as EventEmitter & {
		columns: number;
		rows: number;
	};
	emitter.columns = 120;
	emitter.rows = 40;
	return emitter as unknown as NodeJS.WriteStream;
}

describe("end-to-end smoke", () => {
	it("a queued task runs to done and the TUI shows it", async () => {
		const { server, store, engine, sock, base } = await startServer({
			worktrees: [{ name: "main", path: "/wt/main", branch: "main" }],
		});
		const app = render(
			<App
				sockPath={sock}
				runsDir={`${base}/runs`}
				actions={createActions(sock)}
				stdoutStream={bigStream()}
			/>,
		);
		cleanups.push(() => app.unmount());
		store.create({
			prompt: "run me",
			repo: "platform",
			ref: "worktree:main",
			source: "tui",
		});
		await engine.tick(); // resolve
		await engine.tick(); // start
		await engine.drain();
		server.broadcast();
		await wait(250);
		expect(app.lastFrame()).toContain("✓");
		expect(app.lastFrame()).toContain("run me");
	});
});
