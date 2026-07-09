import { EventEmitter } from "node:events";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { render } from "ink-testing-library";
import { afterEach, describe, expect, it } from "vitest";
import { App } from "../App.js";
import type { Actions, DefinitionSummary, EnqueueOptions } from "../actions.js";
import { createActions } from "../actions.js";
import { cleanups, startServer } from "./helpers.js";

afterEach(async () => {
	while (cleanups.length) await cleanups.pop()?.();
});

const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

const CTRL_S = "\u0013";
const UP = "\u001b[A";
const DOWN = "\u001b[B";
const LEFT = "\u001b[D";
const RIGHT = "\u001b[C";
const ESC = "\u001b";

type FakeStream = EventEmitter & { columns: number; rows: number };
function fakeStream(columns: number, rows: number): FakeStream {
	const emitter = new EventEmitter() as FakeStream;
	emitter.columns = columns;
	emitter.rows = rows;
	return emitter;
}

// A fixed, generous terminal so layout tests do not depend on the ambient
// terminal (App pins the root Box to the reported terminal size).
const big = () => fakeStream(120, 40) as unknown as NodeJS.WriteStream;

interface FakeCalls {
	enqueue: [string, string, EnqueueOptions | undefined][];
	runDefinition: [string, string, string[], string | undefined][];
	retry: string[];
	skip: string[];
	setWorktree: [string, string][];
}

function fakeActions(defs: DefinitionSummary[] = []): {
	actions: Actions;
	calls: FakeCalls;
} {
	const calls: FakeCalls = {
		enqueue: [],
		runDefinition: [],
		retry: [],
		skip: [],
		setWorktree: [],
	};
	const actions: Actions = {
		enqueue: async (prompt, repo, opts) => {
			calls.enqueue.push([prompt, repo, opts]);
			return null;
		},
		retry: async (id) => {
			calls.retry.push(id);
			return null;
		},
		skip: async (id) => {
			calls.skip.push(id);
			return null;
		},
		setWorktree: async (id, wt) => {
			calls.setWorktree.push([id, wt]);
			return null;
		},
		runDefinition: async (repo, name, args, worktree) => {
			calls.runDefinition.push([repo, name, args, worktree]);
			return null;
		},
		definition: async () => null,
		definitions: async () => defs,
	};
	return { actions, calls };
}

describe("App full-screen", () => {
	it("renders a tab bar with project names and queue rows", async () => {
		const { store, server, sock, base } = await startServer();
		store.create({
			prompt: "fix the thing",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		server.broadcast();
		const app = render(
			<App
				sockPath={sock}
				runsDir={`${base}/runs`}
				actions={createActions(sock)}
				stdoutStream={big()}
			/>,
		);
		cleanups.push(() => app.unmount());
		await wait(250);
		expect(app.lastFrame()).toContain("1:platform");
		expect(app.lastFrame()).toContain("QUEUE");
		expect(app.lastFrame()).toContain("WORKTREES");
		expect(app.lastFrame()).toContain("fix the thing");
		// header pins running N/M from the daemon snapshot (maxConcurrentTasks: 1)
		expect(app.lastFrame()).toContain("running 0/1");
	});

	it("ctrl+s then 2 switches to the second project tab", async () => {
		const { store, server, sock, base } = await startServer();
		store.create({
			prompt: "platform work",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		store.create({
			prompt: "queohoh work",
			repo: "queohoh",
			ref: "temp",
			source: "tui",
		});
		server.broadcast();
		const app = render(
			<App
				sockPath={sock}
				runsDir={`${base}/runs`}
				actions={createActions(sock)}
				stdoutStream={big()}
			/>,
		);
		cleanups.push(() => app.unmount());
		await wait(250);
		// tab 1: platform — queohoh's task is filtered out
		expect(app.lastFrame()).toContain("platform work");
		expect(app.lastFrame()).not.toContain("queohoh work");
		app.stdin.write(CTRL_S);
		await wait(20);
		app.stdin.write("2");
		await wait(60);
		// tab 2: queohoh (synthetic) — platform's task is filtered out
		expect(app.lastFrame()).toContain("queohoh work");
		expect(app.lastFrame()).not.toContain("platform work");
	});

	it("transcript scroll: ↑ goes into history, ↓ returns toward the live tail", async () => {
		const { store, server, sock, base } = await startServer();
		const task = store.create({
			prompt: "long running task",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		server.broadcast();
		const runsDir = join(base, "runs");
		mkdirSync(join(runsDir, task.id), { recursive: true });
		const lines = Array.from({ length: 40 }, (_, i) => `line-${i}`).join("\n");
		writeFileSync(join(runsDir, task.id, "transcript.md"), `${lines}\n`);
		const app = render(
			<App
				sockPath={sock}
				runsDir={runsDir}
				actions={createActions(sock)}
				stdoutStream={big()}
			/>,
		);
		cleanups.push(() => app.unmount());
		await wait(300);
		// focus starts on queue — footer shows queue hints
		expect(app.lastFrame()).toContain("[enter] detail");
		app.stdin.write(CTRL_S);
		await wait(20);
		app.stdin.write(RIGHT);
		await wait(60);
		// focus detail — footer shows scroll hints; the transcript defaults to the
		// bottom-anchored tail (newest lines visible, oldest scrolled off)
		expect(app.lastFrame()).toContain("[g/G] top/bottom");
		expect(app.lastFrame()).toContain("line-39");
		expect(app.lastFrame()).not.toContain("line-5");
		// ↓ at the live tail is a no-op — it does not scroll into history
		app.stdin.write(DOWN);
		await wait(60);
		expect(app.lastFrame()).toContain("line-39");
		expect(app.lastFrame()).not.toContain("line-5");
		// ↑ scrolls into history: older lines appear, the tail scrolls off
		app.stdin.write(UP);
		app.stdin.write(UP);
		await wait(60);
		expect(app.lastFrame()).toContain("line-5");
		expect(app.lastFrame()).not.toContain("line-39");
		// ↓ moves back toward the live tail
		app.stdin.write(DOWN);
		await wait(60);
		expect(app.lastFrame()).toContain("line-39");
		app.stdin.write(CTRL_S);
		await wait(20);
		app.stdin.write(LEFT);
		await wait(60);
		// focus back on queue
		expect(app.lastFrame()).toContain("[enter] detail");
	});

	it("top-anchored view: G jumps to the last line, g back to the first", async () => {
		const { store, server, sock, base } = await startServer();
		// Zero-padded, fixed-width markers so no line is a substring of another.
		// LN000 (the first line) is also the queue-row prompt summary, so we
		// assert on LN001 (detail-only) for the head checks.
		const promptLines = Array.from(
			{ length: 60 },
			(_, i) => `LN${String(i).padStart(3, "0")}`,
		);
		store.create({
			prompt: `${promptLines.join("\n")}\n`,
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		server.broadcast();
		const app = render(
			<App
				sockPath={sock}
				runsDir={`${base}/runs`}
				actions={createActions(sock)}
				stdoutStream={big()}
			/>,
		);
		cleanups.push(() => app.unmount());
		await wait(300);
		// focus detail, then switch to the top-anchored "prompt" sub-tab (3).
		app.stdin.write(CTRL_S);
		await wait(20);
		app.stdin.write(RIGHT);
		await wait(40);
		app.stdin.write("3");
		await wait(60);
		// top-anchored default shows the head: early line visible, last is not.
		expect(app.lastFrame()).toContain("LN001");
		expect(app.lastFrame()).not.toContain("LN059");
		// G jumps to the tail/end: last line visible, the head scrolled off.
		app.stdin.write("G");
		await wait(60);
		expect(app.lastFrame()).toContain("LN059");
		expect(app.lastFrame()).not.toContain("LN001");
		// g returns to the head/oldest: early line visible again.
		app.stdin.write("g");
		await wait(60);
		expect(app.lastFrame()).toContain("LN001");
		expect(app.lastFrame()).not.toContain("LN059");
	});

	// The queue `a` → add-prompt binding was removed; adding now originates from
	// the worktrees pane (f/m → worktree-add). On the queue, `a` is inert: it must
	// not open the prompt modal or enqueue anything.
	it("a on the queue is a no-op (add moved to worktrees f/m)", async () => {
		const { sock, base } = await startServer();
		const { actions, calls } = fakeActions();
		const app = render(
			<App
				sockPath={sock}
				runsDir={`${base}/runs`}
				actions={actions}
				stdoutStream={big()}
			/>,
		);
		cleanups.push(() => app.unmount());
		await wait(250);
		app.stdin.write("a");
		await wait(40);
		expect(app.lastFrame()).not.toContain("prompt>");
		for (const ch of "do a thing") app.stdin.write(ch);
		app.stdin.write("\r");
		await wait(80);
		expect(calls.enqueue).toEqual([]);
	});

	it("tasks pane: enter on a def with args opens args input; on a def without args runs it", async () => {
		const { sock, base } = await startServer();
		const defs: DefinitionSummary[] = [
			{ repo: "platform", name: "withargs", args: ["id"], hasDiscovery: false },
			{ repo: "platform", name: "noargs", args: [], hasDiscovery: false },
		];
		const { actions, calls } = fakeActions(defs);
		const app = render(
			<App
				sockPath={sock}
				runsDir={`${base}/runs`}
				actions={actions}
				stdoutStream={big()}
			/>,
		);
		cleanups.push(() => app.unmount());
		await wait(250);
		app.stdin.write(CTRL_S);
		await wait(20);
		app.stdin.write(DOWN); // focus tasks
		await wait(60);
		app.stdin.write("\r"); // enter on withargs
		await wait(60);
		expect(app.lastFrame()).toContain("withargs args");
		app.stdin.write(ESC); // esc
		await wait(40);
		app.stdin.write(DOWN); // select noargs
		await wait(40);
		app.stdin.write("\r"); // enter on noargs
		await wait(80);
		expect(calls.runDefinition).toEqual([
			["platform", "noargs", [], undefined],
		]);
	});

	it("worktrees pane: enter opens the def picker; picking a def runs it against the worktree", async () => {
		const { engine, server, sock, base } = await startServer({
			worktrees: [{ name: "wt-a", path: "/wt/wt-a", branch: "wt-a" }],
		});
		await engine.tick();
		server.broadcast();
		const defs: DefinitionSummary[] = [
			{ repo: "platform", name: "autotest", args: [], hasDiscovery: false },
		];
		const { actions, calls } = fakeActions(defs);
		const app = render(
			<App
				sockPath={sock}
				runsDir={`${base}/runs`}
				actions={actions}
				stdoutStream={big()}
			/>,
		);
		cleanups.push(() => app.unmount());
		await wait(300);
		expect(app.lastFrame()).toContain("wt-a");
		app.stdin.write(CTRL_S);
		await wait(20);
		app.stdin.write(DOWN); // queue -> tasks
		await wait(30);
		app.stdin.write(CTRL_S);
		await wait(20);
		app.stdin.write(DOWN); // tasks -> worktrees
		await wait(40);
		app.stdin.write("\r"); // open def picker
		await wait(60);
		expect(app.lastFrame()).toContain("Run task definition");
		expect(app.lastFrame()).toContain("wt-a");
		app.stdin.write("\r"); // pick autotest
		await wait(80);
		expect(calls.runDefinition).toEqual([["platform", "autotest", [], "wt-a"]]);
	});

	// Navigate focus from the default queue pane down to the worktrees pane.
	const focusWorktrees = async (app: ReturnType<typeof render>) => {
		app.stdin.write(CTRL_S);
		await wait(20);
		app.stdin.write(DOWN); // queue -> tasks
		await wait(30);
		app.stdin.write(CTRL_S);
		await wait(20);
		app.stdin.write(DOWN); // tasks -> worktrees
		await wait(40);
	};

	it("worktrees pane: f opens the fresh-session add-task modal and enqueues with the worktree", async () => {
		const { engine, server, sock, base } = await startServer({
			worktrees: [{ name: "wt-a", path: "/wt/wt-a", branch: "wt-a" }],
		});
		await engine.tick();
		server.broadcast();
		const { actions, calls } = fakeActions();
		const app = render(
			<App
				sockPath={sock}
				runsDir={`${base}/runs`}
				actions={actions}
				stdoutStream={big()}
			/>,
		);
		cleanups.push(() => app.unmount());
		await wait(300);
		await focusWorktrees(app);
		app.stdin.write("f");
		await wait(60);
		// Titled modal, centered over the still-visible body panes (spike outcome).
		expect(app.lastFrame()).toContain(
			"New task — fresh session — platform:wt-a",
		);
		expect(app.lastFrame()).toContain("QUEUE");
		for (const ch of "do a thing") app.stdin.write(ch);
		app.stdin.write("\r");
		await wait(80);
		expect(calls.enqueue).toEqual([
			["do a thing", "platform", { worktree: "wt-a", session: "fresh" }],
		]);
	});

	it("worktrees pane: m opens the main-session add-task modal and enqueues session main", async () => {
		const { engine, server, sock, base } = await startServer({
			worktrees: [{ name: "wt-a", path: "/wt/wt-a", branch: "wt-a" }],
		});
		await engine.tick();
		server.broadcast();
		const { actions, calls } = fakeActions();
		const app = render(
			<App
				sockPath={sock}
				runsDir={`${base}/runs`}
				actions={actions}
				stdoutStream={big()}
			/>,
		);
		cleanups.push(() => app.unmount());
		await wait(300);
		await focusWorktrees(app);
		app.stdin.write("m");
		await wait(60);
		expect(app.lastFrame()).toContain(
			"New task — main session — platform:wt-a",
		);
		for (const ch of "ship it") app.stdin.write(ch);
		app.stdin.write("\r");
		await wait(80);
		expect(calls.enqueue).toEqual([
			["ship it", "platform", { worktree: "wt-a", session: "main" }],
		]);
	});

	it("add-task modal: typing q inserts a literal q; esc cancels without enqueue", async () => {
		const { engine, server, sock, base } = await startServer({
			worktrees: [{ name: "wt-a", path: "/wt/wt-a", branch: "wt-a" }],
		});
		await engine.tick();
		server.broadcast();
		const { actions, calls } = fakeActions();
		const app = render(
			<App
				sockPath={sock}
				runsDir={`${base}/runs`}
				actions={actions}
				stdoutStream={big()}
			/>,
		);
		cleanups.push(() => app.unmount());
		await wait(300);
		await focusWorktrees(app);
		app.stdin.write("f");
		await wait(60);
		app.stdin.write("q"); // must insert into the text input, not close it
		await wait(60);
		expect(app.lastFrame()).toContain("prompt> q");
		app.stdin.write(ESC); // esc cancels the text modal
		await wait(60);
		expect(app.lastFrame()).not.toContain("New task —");
		expect(calls.enqueue).toEqual([]);
	});

	it("def-pick modal closes on q and on esc", async () => {
		const { engine, server, sock, base } = await startServer({
			worktrees: [{ name: "wt-a", path: "/wt/wt-a", branch: "wt-a" }],
		});
		await engine.tick();
		server.broadcast();
		const defs: DefinitionSummary[] = [
			{ repo: "platform", name: "autotest", args: [], hasDiscovery: false },
		];
		const { actions } = fakeActions(defs);
		const app = render(
			<App
				sockPath={sock}
				runsDir={`${base}/runs`}
				actions={actions}
				stdoutStream={big()}
			/>,
		);
		cleanups.push(() => app.unmount());
		await wait(300);
		await focusWorktrees(app);
		app.stdin.write("\r"); // open def picker
		await wait(60);
		expect(app.lastFrame()).toContain("Run task definition");
		app.stdin.write("q"); // q closes the picker
		await wait(60);
		expect(app.lastFrame()).not.toContain("Run task definition");
		app.stdin.write("\r"); // reopen
		await wait(60);
		expect(app.lastFrame()).toContain("Run task definition");
		app.stdin.write(ESC); // esc also closes
		await wait(60);
		expect(app.lastFrame()).not.toContain("Run task definition");
	});

	it("q exits — input stops being handled", async () => {
		const { sock, base } = await startServer();
		const app = render(
			<App
				sockPath={sock}
				runsDir={`${base}/runs`}
				actions={createActions(sock)}
				stdoutStream={big()}
			/>,
		);
		cleanups.push(() => app.unmount());
		await wait(250);
		app.stdin.write("q");
		await wait(60);
		app.stdin.write("a"); // would open prompt if still alive
		await wait(60);
		expect(app.lastFrame()).not.toContain("prompt>");
	});

	it("renders the tiny-terminal guard below 60x15", async () => {
		const { sock, base } = await startServer();
		const app = render(
			<App
				sockPath={sock}
				runsDir={`${base}/runs`}
				actions={createActions(sock)}
				stdoutStream={fakeStream(40, 10) as unknown as NodeJS.WriteStream}
			/>,
		);
		cleanups.push(() => app.unmount());
		await wait(150);
		expect(app.lastFrame()).toContain("terminal too small (60x15 minimum)");
	});

	it("shows the daemon-unreachable banner without a daemon", async () => {
		const app = render(
			<App
				sockPath="/tmp/qo-no-daemon"
				runsDir="/tmp/qo-no-runs"
				actions={createActions("/tmp/qo-no-daemon")}
				stdoutStream={big()}
			/>,
		);
		cleanups.push(() => app.unmount());
		await wait(200);
		expect(app.lastFrame()).toContain("daemon unreachable");
	});
});
