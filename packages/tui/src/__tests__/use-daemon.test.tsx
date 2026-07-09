import { mkdtempSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Text } from "ink";
import { render as tlRender } from "ink-testing-library";
import { Profiler } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { createActions } from "../actions.js";
import { normalizeSnapshot, useDaemon } from "../use-daemon.js";
import { cleanups, makeSnapshot, startServer } from "./helpers.js";

/**
 * Minimal fake daemon speaking the line protocol but serving an arbitrary
 * (possibly OLD-SHAPE) snapshot, so we can exercise the socket boundary with
 * data the current compile-time `StateSnapshot` type doesn't actually match.
 */
async function startOldDaemon(
	oldSnapshot: Record<string, unknown>,
): Promise<{ sock: string }> {
	const base = mkdtempSync(join(tmpdir(), "qo-old-"));
	const sock = join(base, "d.sock");
	const server = createServer((socket) => {
		let buffer = "";
		socket.on("data", (chunk) => {
			buffer += chunk.toString();
			const lines = buffer.split("\n");
			buffer = lines.pop() ?? "";
			for (const line of lines) {
				if (!line.trim()) continue;
				const frame = JSON.parse(line) as { id: number; method: string };
				const result = frame.method === "state" ? oldSnapshot : null;
				socket.write(`${JSON.stringify({ id: frame.id, result })}\n`);
			}
		});
	});
	await new Promise<void>((resolve) => server.listen(sock, () => resolve()));
	cleanups.push(
		() => new Promise<void>((resolve) => server.close(() => resolve())),
	);
	return { sock };
}

/** Reads normalized fields WITHOUT `??` guards so an un-normalized snapshot
 * (missing `projects`/`worktrees`) crashes the render — the regression. */
function OldProbe({ sock }: { sock: string }) {
	const { snapshot } = useDaemon(sock, { retryMs: 100 });
	if (!snapshot) return <Text>pending</Text>;
	return (
		<Text>
			p={snapshot.projects.length},w={Object.keys(snapshot.worktrees).length},t=
			{snapshot.tasks.length}
		</Text>
	);
}

afterEach(async () => {
	while (cleanups.length) await cleanups.pop()?.();
});

function Probe({ sock }: { sock: string }) {
	const { snapshot, connected } = useDaemon(sock, { retryMs: 100 });
	return (
		<Text>
			{connected ? "connected" : "disconnected"}:{snapshot?.tasks.length ?? -1}
		</Text>
	);
}

const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

describe("useDaemon", () => {
	it("connects, receives initial state, and sees pushed updates", async () => {
		const { server, store, sock } = await startServer();
		const app = tlRender(<Probe sock={sock} />);
		cleanups.push(() => app.unmount());
		await wait(200);
		expect(app.lastFrame()).toContain("connected:0");
		store.create({
			prompt: "hi",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		server.broadcast();
		await wait(200);
		expect(app.lastFrame()).toContain("connected:1");
	});

	it("does not re-render on a content-identical re-broadcast", async () => {
		const { server, store, sock } = await startServer();
		let renders = 0;
		const app = tlRender(
			<Profiler
				id="daemon"
				onRender={() => {
					renders += 1;
				}}
			>
				<Probe sock={sock} />
			</Profiler>,
		);
		cleanups.push(() => app.unmount());
		await wait(200); // connect + initial state
		// A real content change commits (and renders).
		store.create({
			prompt: "hi",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		server.broadcast();
		await wait(150);
		expect(app.lastFrame()).toContain("connected:1");
		const afterChange = renders;
		// Two byte-identical re-broadcasts (the daemon's idle cadence) must be
		// deduped: no state update, no render.
		server.broadcast();
		server.broadcast();
		await wait(150);
		expect(renders).toBe(afterChange);
		// A subsequent real change still commits, proving dedup didn't wedge us.
		store.create({
			prompt: "yo",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		server.broadcast();
		await wait(150);
		expect(app.lastFrame()).toContain("connected:2");
		expect(renders).toBeGreaterThan(afterChange);
	});

	it("reports disconnected when no daemon and keeps retrying without crashing", async () => {
		const app = tlRender(<Probe sock="/tmp/qo-nonexistent-sock" />);
		cleanups.push(() => app.unmount());
		await wait(300);
		expect(app.lastFrame()).toContain("disconnected:-1");
	});

	it("normalizes an OLD-daemon snapshot missing projects/worktrees/maxConcurrent without crashing", async () => {
		// An older daemon build predates these fields — the wire snapshot only
		// carries tasks/archivedRecent/sessions/running.
		const { sock } = await startOldDaemon({
			tasks: [{ id: "t1" }],
			archivedRecent: [],
			sessions: [],
			running: [],
		});
		const app = tlRender(<OldProbe sock={sock} />);
		cleanups.push(() => app.unmount());
		await wait(200);
		// projects -> [], worktrees -> {}, tasks passthrough: no `.map`/`.length`
		// on undefined blows up.
		expect(app.lastFrame()).toContain("p=0,w=0,t=1");
	});
});

describe("normalizeSnapshot", () => {
	it("fills missing fields with safe defaults and keeps maxConcurrent nullish", () => {
		const s = normalizeSnapshot({
			tasks: [],
			archivedRecent: [],
			sessions: [],
			running: [],
		});
		expect(s.projects).toEqual([]);
		expect(s.worktrees).toEqual({});
		expect(s.sessions).toEqual([]);
		expect(s.running).toEqual([]);
		// Old daemons never sent maxConcurrent; App does `?? null`, so the header
		// must still omit "/M". The normalized value stays nullish, NOT 0.
		expect(s.maxConcurrent ?? null).toBeNull();
	});

	it("coerces wrong-typed fields (null/non-array/non-object) to defaults", () => {
		const s = normalizeSnapshot({
			tasks: null,
			projects: "nope",
			worktrees: [],
			running: undefined,
		});
		expect(s.tasks).toEqual([]);
		expect(s.projects).toEqual([]);
		expect(s.worktrees).toEqual({});
		expect(s.running).toEqual([]);
	});

	it("passes a full modern snapshot through unchanged", () => {
		const full = makeSnapshot({
			maxConcurrent: 3,
			projects: [{ name: "p" }],
			worktrees: { p: [] },
		});
		const s = normalizeSnapshot(full);
		expect(s.maxConcurrent).toBe(3);
		expect(s.projects).toEqual([{ name: "p" }]);
		expect(s.worktrees).toEqual({ p: [] });
	});

	it("tolerates a nullish raw value", () => {
		expect(normalizeSnapshot(null).projects).toEqual([]);
		expect(normalizeSnapshot(undefined).worktrees).toEqual({});
	});
});

describe("createActions", () => {
	it("enqueue succeeds and errors resolve as message strings", async () => {
		const { store, sock } = await startServer();
		const actions = createActions(sock);
		expect(await actions.enqueue("fix it", "platform")).toBeNull();
		expect(store.list()).toHaveLength(1);
		const bad = await actions.retry(store.list()[0]?.id ?? "");
		expect(bad).toMatch(/cannot retry/);
	});
});
