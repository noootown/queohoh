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
const WHEEL_UP = "\u001b[<64;5;5M";
const WHEEL_DOWN = "\u001b[<65;5;5M";

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
	createWorktree: [string, string][];
}

function fakeActions(
	defs: DefinitionSummary[] = [],
	createWorktreeResult: string | null = null,
): {
	actions: Actions;
	calls: FakeCalls;
} {
	const calls: FakeCalls = {
		enqueue: [],
		runDefinition: [],
		retry: [],
		skip: [],
		setWorktree: [],
		createWorktree: [],
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
		removeWorktree: async () => null,
		createWorktree: async (repo, name) => {
			calls.createWorktree.push([repo, name]);
			return createWorktreeResult;
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

	// The queue `a` → add-prompt binding was removed; `a` now opens the action
	// menu, and adding a task originates from a worktree's menu. On an empty queue
	// nothing is selected, so `a` is inert: it must not open the prompt modal or
	// enqueue anything.
	it("a on an empty queue is inert (nothing selected)", async () => {
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
		expect(app.lastFrame()).not.toContain("enter run · esc close");
		for (const ch of "do a thing") app.stdin.write(ch);
		app.stdin.write("\r");
		await wait(80);
		expect(calls.enqueue).toEqual([]);
	});

	it("tasks pane: menu Run on a def with args opens args input; on a def without args runs it", async () => {
		const { sock, base } = await startServer();
		const defs: DefinitionSummary[] = [
			{
				repo: "platform",
				name: "withargs",
				scope: "project",
				args: [{ name: "id" }],
				hasDiscovery: false,
			},
			{
				repo: "platform",
				name: "noargs",
				scope: "project",
				args: [],
				hasDiscovery: false,
			},
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
		app.stdin.write("a"); // open action menu (withargs selected)
		await wait(60);
		app.stdin.write("\r"); // Run (index 0)
		await wait(60);
		expect(app.lastFrame()).toContain("withargs args");
		app.stdin.write(ESC); // esc closes the args input
		await wait(40);
		app.stdin.write(DOWN); // select noargs
		await wait(40);
		app.stdin.write("a"); // open action menu (noargs selected)
		await wait(60);
		app.stdin.write("\r"); // Run (index 0)
		await wait(80);
		expect(calls.runDefinition).toEqual([
			["platform", "noargs", [], undefined],
		]);
	});

	it("worktrees pane: menu Run task definition… opens the def picker; picking a def runs it against the worktree", async () => {
		const { engine, server, sock, base } = await startServer({
			worktrees: [{ name: "wt-a", path: "/wt/wt-a", branch: "wt-a" }],
		});
		await engine.tick();
		server.broadcast();
		const defs: DefinitionSummary[] = [
			{
				repo: "platform",
				name: "autotest",
				scope: "project",
				args: [],
				hasDiscovery: false,
			},
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
		app.stdin.write("a"); // open action menu
		await wait(60);
		// Run task definition… is the 3rd row (index 2): j×2.
		app.stdin.write("j"); // -> New task (main session)…
		await wait(20);
		app.stdin.write("j"); // -> Run task definition…
		await wait(20);
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

	it("worktrees pane: menu New task (fresh session)… opens the add-task modal and enqueues with the worktree", async () => {
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
		app.stdin.write("a"); // open action menu
		await wait(60);
		app.stdin.write("\r"); // New task (fresh session)… (index 0)
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

	it("worktrees pane: menu New task (main session)… opens the add-task modal and enqueues session main", async () => {
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
		app.stdin.write("a"); // open action menu
		await wait(60);
		app.stdin.write("j"); // -> New task (main session)… (index 1)
		await wait(40);
		app.stdin.write("\r");
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
		app.stdin.write("a"); // open action menu
		await wait(60);
		app.stdin.write("\r"); // New task (fresh session)… (index 0)
		await wait(60);
		app.stdin.write("q"); // must insert into the text input, not close it
		await wait(60);
		expect(app.lastFrame()).toContain("prompt> q");
		app.stdin.write(ESC); // esc cancels the text modal
		await wait(60);
		expect(app.lastFrame()).not.toContain("New task —");
		expect(calls.enqueue).toEqual([]);
	});

	it("worktrees pane: c opens the create-worktree modal and submits a valid branch", async () => {
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
		app.stdin.write("c"); // open create-worktree modal
		await wait(60);
		expect(app.lastFrame()).toContain("Create worktree — platform");
		for (const ch of "feature-x") app.stdin.write(ch);
		app.stdin.write("\r");
		await wait(80);
		expect(calls.createWorktree).toEqual([["platform", "feature-x"]]);
		expect(app.lastFrame()).not.toContain("Create worktree —");
	});

	it("create-worktree modal: a mouse click report does not leak into the input value", async () => {
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
		app.stdin.write("c"); // open create-worktree modal
		await wait(60);
		for (const ch of "feat") app.stdin.write(ch);
		await wait(30);
		// Mouse tracking is on while the modal floats; a click arrives as an SGR
		// report with ESC stripped by ink. It must not append its coordinates.
		app.stdin.write("[<0;34;12M");
		app.stdin.write("[<0;34;12m");
		await wait(40);
		expect(app.lastFrame()).toContain("branch> feat");
		expect(app.lastFrame()).not.toContain("34;12"); // distinctive coordinate leak
		expect(app.lastFrame()).not.toContain("feat[<");
		// submitting still carries only the typed value
		app.stdin.write("\r");
		await wait(60);
		expect(calls.createWorktree).toEqual([["platform", "feat"]]);
	});

	it("create-worktree modal: an invalid branch shows an inline error and keeps the input", async () => {
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
		app.stdin.write("c"); // open create-worktree modal
		await wait(60);
		for (const ch of "bad name") app.stdin.write(ch);
		app.stdin.write("\r");
		await wait(80);
		expect(app.lastFrame()).toContain("no whitespace allowed");
		expect(app.lastFrame()).toContain("branch> bad name");
		expect(calls.createWorktree).toEqual([]);
	});

	it("create-worktree modal: submit closes immediately and a backend error lands on the status line", async () => {
		const { engine, server, sock, base } = await startServer({
			worktrees: [{ name: "wt-a", path: "/wt/wt-a", branch: "wt-a" }],
		});
		await engine.tick();
		server.broadcast();
		const { actions, calls } = fakeActions([], "wt exited with code 1");
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
		app.stdin.write("c"); // open create-worktree modal
		await wait(60);
		for (const ch of "feature-x") app.stdin.write(ch);
		app.stdin.write("\r");
		await wait(80);
		// Modal must not block on the (potentially minutes-long) creation.
		expect(app.lastFrame()).not.toContain("Create worktree —");
		expect(calls.createWorktree).toEqual([["platform", "feature-x"]]);
		expect(app.lastFrame()).toContain(
			"create worktree feature-x: wt exited with code 1",
		);
	});

	it("worktrees pane: menu Create worktree… opens the same modal", async () => {
		const { engine, server, sock, base } = await startServer({
			worktrees: [{ name: "wt-a", path: "/wt/wt-a", branch: "wt-a" }],
		});
		await engine.tick();
		server.broadcast();
		const { actions } = fakeActions();
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
		app.stdin.write("a"); // open action menu
		await wait(60);
		for (let i = 0; i < 6; i += 1) {
			app.stdin.write("j"); // step down to Create worktree… (index 6)
			await wait(20);
		}
		app.stdin.write("\r");
		await wait(60);
		expect(app.lastFrame()).toContain("Create worktree — platform");
	});

	it("queue pane: c opens the adhoc add-task modal and enqueues with no worktree", async () => {
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
		app.stdin.write("c"); // queue is focused by default -> adhoc add-task
		await wait(60);
		expect(app.lastFrame()).toContain(
			"New task — fresh session — platform (adhoc)",
		);
		for (const ch of "run this now") app.stdin.write(ch);
		app.stdin.write("\r");
		await wait(80);
		expect(calls.enqueue).toEqual([
			["run this now", "platform", { worktree: "", session: "fresh" }],
		]);
	});

	it("def-pick modal closes on q and on esc", async () => {
		const { engine, server, sock, base } = await startServer({
			worktrees: [{ name: "wt-a", path: "/wt/wt-a", branch: "wt-a" }],
		});
		await engine.tick();
		server.broadcast();
		const defs: DefinitionSummary[] = [
			{
				repo: "platform",
				name: "autotest",
				scope: "project",
				args: [],
				hasDiscovery: false,
			},
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
		// Open the def picker via the worktree action menu (Run task definition…).
		const openDefPicker = async () => {
			app.stdin.write("a"); // open action menu
			await wait(60);
			// Run task definition… is the 3rd row (index 2): j×2.
			app.stdin.write("j"); // -> New task (main session)…
			await wait(20);
			app.stdin.write("j"); // -> Run task definition…
			await wait(20);
			app.stdin.write("\r"); // open def picker
			await wait(60);
		};
		await wait(300);
		await focusWorktrees(app);
		await openDefPicker();
		expect(app.lastFrame()).toContain("Run task definition");
		app.stdin.write("q"); // q closes the picker
		await wait(60);
		expect(app.lastFrame()).not.toContain("Run task definition");
		await openDefPicker(); // reopen
		expect(app.lastFrame()).toContain("Run task definition");
		app.stdin.write(ESC); // esc also closes
		await wait(60);
		expect(app.lastFrame()).not.toContain("Run task definition");
	});

	it("def-pick renders arg defaults and a (g) marker for global defs", async () => {
		const { engine, server, sock, base } = await startServer({
			worktrees: [{ name: "wt-a", path: "/wt/wt-a", branch: "wt-a" }],
		});
		await engine.tick();
		server.broadcast();
		const defs: DefinitionSummary[] = [
			{
				repo: "platform",
				name: "pr-ready",
				scope: "project",
				args: [{ name: "pr" }, { name: "mode", default: "ready" }],
				hasDiscovery: false,
			},
			{
				repo: "platform",
				name: "squash-merge",
				scope: "global",
				args: [{ name: "source" }, { name: "target", default: "main" }],
				hasDiscovery: false,
			},
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
		app.stdin.write("a"); // open action menu
		await wait(60);
		app.stdin.write("j"); // -> New task (main session)…
		await wait(20);
		app.stdin.write("j"); // -> Run task definition…
		await wait(20);
		app.stdin.write("\r"); // open def picker
		await wait(60);
		const frame = app.lastFrame() ?? "";
		expect(frame).toContain("pr-ready (pr, mode=ready)");
		expect(frame).toContain("squash-merge (source, target=main)");
		expect(frame).toContain("(g)"); // global marker on the squash-merge row
	});

	it("worktree menu: Squash merge into… opens def-args prefilled with the branch and runs without a worktree override", async () => {
		const { engine, server, sock, base } = await startServer({
			worktrees: [{ name: "wt-a", path: "/wt/wt-a", branch: "wt-a" }],
		});
		await engine.tick();
		server.broadcast();
		const defs: DefinitionSummary[] = [
			{
				repo: "platform",
				name: "squash-merge",
				scope: "global",
				args: [{ name: "source" }, { name: "target", default: "main" }],
				hasDiscovery: false,
			},
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
		await focusWorktrees(app);
		app.stdin.write("a"); // open action menu
		await wait(60);
		// Squash merge into… is the 5th row (index 4, above Remove worktree…): j×4.
		for (let i = 0; i < 4; i += 1) {
			app.stdin.write("j");
			await wait(20);
		}
		app.stdin.write("\r"); // select -> fetch defs -> open def-args
		await wait(80);
		expect(app.lastFrame()).toContain("squash-merge args"); // modal title
		expect(app.lastFrame()).toContain("source> wt-a"); // fixed, shown read-only
		// `source` is fixed (not asked): focus starts on `target`, so typing edits
		// the target field, never the source.
		for (let i = 0; i < 4; i += 1) app.stdin.write("\u007f"); // DEL -> backspace: clear "main"
		for (const ch of "dev") app.stdin.write(ch);
		await wait(40);
		expect(app.lastFrame()).toContain("source> wt-a");
		expect(app.lastFrame()).toContain("target> dev");
		app.stdin.write("\r"); // submit (source=wt-a fixed, target=dev typed)
		await wait(80);
		// worktree override is undefined — the def's `worktree: repo` governs.
		expect(calls.runDefinition).toEqual([
			["platform", "squash-merge", ["wt-a", "dev"], undefined],
		]);
	});

	it("worktree menu: Squash merge into… is disabled with a reason while a task runs in the worktree", async () => {
		const { store, engine, server, sock, base } = await startServer({
			worktrees: [{ name: "wt-a", path: "/wt/wt-a", branch: "wt-a" }],
		});
		await engine.tick();
		const task = store.create({
			prompt: "busy work",
			repo: "platform",
			ref: "worktree:wt-a",
			source: "tui",
		});
		store.update(task.id, {
			status: "running",
			target: { repo: "platform", ref: "worktree:wt-a", worktree: "wt-a" },
		});
		server.broadcast();
		const { actions } = fakeActions();
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
		app.stdin.write("a"); // open action menu
		await wait(60);
		expect(app.lastFrame()).toContain(
			"Squash merge into… — a task is running here",
		);
	});

	it("worktree menu: Squash merge into… surfaces a status line when the global def is absent", async () => {
		const { engine, server, sock, base } = await startServer({
			worktrees: [{ name: "wt-a", path: "/wt/wt-a", branch: "wt-a" }],
		});
		await engine.tick();
		server.broadcast();
		const { actions, calls } = fakeActions([]); // no squash-merge definition
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
		app.stdin.write("a"); // open action menu
		await wait(60);
		for (let i = 0; i < 4; i += 1) {
			app.stdin.write("j");
			await wait(20);
		}
		app.stdin.write("\r"); // select -> definitions() empty -> status line
		await wait(80);
		expect(app.lastFrame()).toContain("squash-merge definition not found");
		expect(calls.runDefinition).toEqual([]);
	});

	it("keeps the left column width stable when focus/detail content changes", async () => {
		const { store, server, engine, sock, base } = await startServer({
			worktrees: [{ name: "wt-a", path: "/wt/wt-a", branch: "wt-a" }],
		});
		const task = store.create({
			prompt: "fix the thing",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		await engine.tick();
		server.broadcast();
		// A very wide, unbreakable detail line: with the old flex-shrink bug this
		// pushed the detail pane wider and shrank the left column.
		const runsDir = join(base, "runs");
		mkdirSync(join(runsDir, task.id), { recursive: true });
		writeFileSync(
			join(runsDir, task.id, "transcript.md"),
			`${"x".repeat(300)}\n`,
		);
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
		// queue selected → detail shows the wide transcript line
		const detailXWithWideContent = app.lastFrame()?.indexOf("DETAIL") ?? -1;
		expect(detailXWithWideContent).toBeGreaterThan(0);
		// move focus to worktrees → detail shows the narrow worktree info
		app.stdin.write(CTRL_S);
		await wait(20);
		app.stdin.write(DOWN); // queue -> tasks
		await wait(30);
		app.stdin.write(CTRL_S);
		await wait(20);
		app.stdin.write(DOWN); // tasks -> worktrees
		await wait(60);
		const detailXWithNarrowContent = app.lastFrame()?.indexOf("DETAIL") ?? -1;
		expect(detailXWithNarrowContent).toBe(detailXWithWideContent);
	});

	it("mouse wheel scrolls the focused detail pane like j/k", async () => {
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
		// focus the detail pane; transcript defaults to the bottom-anchored tail
		app.stdin.write(CTRL_S);
		await wait(20);
		app.stdin.write(RIGHT);
		await wait(60);
		expect(app.lastFrame()).toContain("line-39");
		expect(app.lastFrame()).not.toContain("line-5");
		// wheel up scrolls into history (older lines appear, the tail scrolls off)
		app.stdin.write(WHEEL_UP);
		app.stdin.write(WHEEL_UP);
		await wait(60);
		expect(app.lastFrame()).toContain("line-5");
		expect(app.lastFrame()).not.toContain("line-39");
		// wheel down returns toward the live tail
		app.stdin.write(WHEEL_DOWN);
		app.stdin.write(WHEEL_DOWN);
		await wait(60);
		expect(app.lastFrame()).toContain("line-39");
	});

	it("detail: long lines are truncated so content never overflows the pane or bleeds into the sub-tab header", async () => {
		const { store, server, sock, base } = await startServer();
		const task = store.create({
			prompt: "wide task",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		server.broadcast();
		const runsDir = join(base, "runs");
		mkdirSync(join(runsDir, task.id), { recursive: true });
		// Lines far wider than the pane: if they wrapped, each would consume
		// several rows and overflow the fixed-height pane up into the tab bar.
		const wide = Array.from({ length: 60 }, () => "y".repeat(300)).join("\n");
		writeFileSync(join(runsDir, task.id, "transcript.md"), `${wide}\n`);
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
		app.stdin.write(CTRL_S);
		await wait(20);
		app.stdin.write(RIGHT); // focus detail
		await wait(60);
		const frame = app.lastFrame() ?? "";
		const frameRows = frame.split("\n");
		// The rendered frame never grows taller than the terminal (no overflow).
		expect(frameRows.length).toBeLessThanOrEqual(40);
		// The sub-tab bar sits on its own row right under the title, intact and
		// free of any scrolled content.
		const titleIdx = frameRows.findIndex((r) => r.includes("DETAIL"));
		const tabRow = frameRows[titleIdx + 1] ?? "";
		expect(tabRow).toContain("1:transcript");
		expect(tabRow).toContain("3:prompt");
		expect(tabRow).not.toContain("y");
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

describe("search filter", () => {
	it("typing after / narrows the queue and shows the query in the pane title", async () => {
		const { store, server, sock, base } = await startServer();
		store.create({
			prompt: "alpha work",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		store.create({
			prompt: "beta work",
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
		// both rows visible before filtering
		expect(app.lastFrame()).toContain("alpha work");
		expect(app.lastFrame()).toContain("beta work");
		app.stdin.write("/");
		await wait(40);
		for (const ch of "alp") app.stdin.write(ch);
		await wait(80);
		const frame = app.lastFrame() ?? "";
		// query echoed in the pane title with the active block cursor
		expect(frame).toContain("QUEUE /alp█");
		// queue narrowed to the matching row only
		expect(frame).toContain("alpha work");
		expect(frame).not.toContain("beta work");
	});

	it("enter commits the filter (title keeps /query, input leaves search mode)", async () => {
		const { store, server, sock, base } = await startServer();
		store.create({
			prompt: "alpha one",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		store.create({
			prompt: "alpha two",
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
		app.stdin.write("/");
		await wait(40);
		for (const ch of "alp") app.stdin.write(ch);
		await wait(80);
		expect(app.lastFrame()).toContain("QUEUE /alp█");
		// enter commits: search mode exits, filter (and its title) persists
		app.stdin.write("\r");
		await wait(60);
		const committed = app.lastFrame() ?? "";
		expect(committed).toContain("QUEUE /alp");
		expect(committed).not.toContain("QUEUE /alp█");
		// back in list mode: j navigates (does not append to the query)
		app.stdin.write("j");
		await wait(60);
		const afterJ = app.lastFrame() ?? "";
		expect(afterJ).toContain("QUEUE /alp");
		expect(afterJ).not.toContain("QUEUE /alpj");
		expect(afterJ).not.toContain("█");
		// both matching rows remain filtered-in
		expect(afterJ).toContain("alpha one");
		expect(afterJ).toContain("alpha two");
	});

	it("esc clears the filter — while typing and after commit", async () => {
		const { store, server, sock, base } = await startServer();
		store.create({
			prompt: "alpha work",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		store.create({
			prompt: "beta work",
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
		// esc while still typing in search mode
		app.stdin.write("/");
		await wait(40);
		for (const ch of "alp") app.stdin.write(ch);
		await wait(60);
		expect(app.lastFrame()).not.toContain("beta work");
		app.stdin.write(ESC);
		await wait(60);
		const cleared = app.lastFrame() ?? "";
		expect(cleared).toContain("alpha work");
		expect(cleared).toContain("beta work");
		expect(cleared).not.toContain("QUEUE /");
		// esc after committing the filter clears it just the same
		app.stdin.write("/");
		await wait(40);
		for (const ch of "alp") app.stdin.write(ch);
		await wait(60);
		app.stdin.write("\r");
		await wait(60);
		expect(app.lastFrame()).not.toContain("beta work");
		app.stdin.write(ESC);
		await wait(60);
		const clearedAgain = app.lastFrame() ?? "";
		expect(clearedAgain).toContain("alpha work");
		expect(clearedAgain).toContain("beta work");
		expect(clearedAgain).not.toContain("QUEUE /");
	});

	it("esc-clearing a committed filter resets selection to the first row", async () => {
		const { store, server, sock, base } = await startServer();
		// Distinct second prompt lines (MARKER-*) that never appear in the queue
		// summaries, so the prompt sub-tab uniquely identifies the selected row.
		store.create({
			prompt: "alpha one\nMARKER-ONE",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		store.create({
			prompt: "alpha two\nMARKER-TWO",
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
		// commit a filter matching both rows, then move selection to the 2nd row
		app.stdin.write("/");
		await wait(40);
		for (const ch of "alp") app.stdin.write(ch);
		await wait(40);
		app.stdin.write("\r"); // commit the filter
		await wait(40);
		app.stdin.write("j"); // select the second filtered row (alpha two)
		await wait(40);
		// esc clears the committed filter and must also reset selection to row 0
		app.stdin.write(ESC);
		await wait(60);
		expect(app.lastFrame()).not.toContain("QUEUE /");
		// the detail prompt sub-tab reflects the selected queue row: a reset
		// selection shows the FIRST row's prompt, not the second's.
		app.stdin.write(CTRL_S);
		await wait(20);
		app.stdin.write(RIGHT); // focus detail
		await wait(40);
		app.stdin.write("3"); // prompt sub-tab
		await wait(60);
		const frame = app.lastFrame() ?? "";
		expect(frame).toContain("MARKER-ONE");
		expect(frame).not.toContain("MARKER-TWO");
	});
});
