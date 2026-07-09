import { EventEmitter } from "node:events";
import { currentBuildId } from "@queohoh/daemon";
import { render } from "ink-testing-library";
import { Profiler } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Actions } from "../actions.js";
import { makeSnapshot, makeTask } from "./helpers.js";

// A mutable holder the mocked useDaemon returns. `state` keeps a stable object
// reference (so App's heal/effect deps stay stable), while its `.snapshot` is
// swapped per test before render. `buildId` matches the on-disk build so the
// self-heal effect classifies the daemon as healthy and never fires performHeal.
const daemon = vi.hoisted(() => ({
	state: { snapshot: null as unknown, connected: true },
}));

vi.mock("../use-daemon.js", () => ({
	useDaemon: () => daemon.state,
	normalizeSnapshot: (x: unknown) => x,
}));

// readRunFiles is mocked so we can (a) count reads and (b) flip content on demand
// to exercise the equality-skip in App's run-files poll.
const runFilesContent = vi.hoisted(() => ({
	value: { report: "r0", transcriptTail: ["a", "b"] } as {
		report: string | null;
		transcriptTail: string[];
	},
}));
const readRunFiles = vi.hoisted(() => vi.fn(() => runFilesContent.value));
vi.mock("../run-files.js", () => ({ readRunFiles }));

// Import App AFTER the mocks are registered.
const { App } = await import("../App.js");

const DOWN = "[B";
const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

function big(): NodeJS.WriteStream {
	const emitter = new EventEmitter() as EventEmitter & {
		columns: number;
		rows: number;
	};
	emitter.columns = 120;
	emitter.rows = 40;
	return emitter as unknown as NodeJS.WriteStream;
}

const noopActions: Actions = {
	enqueue: async () => null,
	retry: async () => null,
	skip: async () => null,
	setWorktree: async () => null,
	removeWorktree: async () => null,
	createWorktree: async () => null,
	runDefinition: async () => null,
	definition: async () => null,
	definitions: async () => [],
};

/** Six queued tasks on `platform` so five ↓ presses move through six distinct
 * selections, each changing the selected run (and thus runTaskId). */
function sixTaskSnapshot() {
	const tasks = Array.from({ length: 6 }, (_, i) =>
		makeTask("queued", {
			id: `01TASK${String(i).padStart(20, "0")}`,
			prompt: `task ${i}`,
			target: { repo: "platform", ref: "temp", worktree: "wt-a" },
		}),
	);
	return makeSnapshot({
		tasks,
		projects: [{ name: "platform" }],
		buildId: currentBuildId(),
	});
}

beforeEach(() => {
	readRunFiles.mockClear();
	runFilesContent.value = { report: "r0", transcriptTail: ["a", "b"] };
});

afterEach(() => {
	vi.useRealTimers();
});

describe("App render pressure", () => {
	it("renders once per arrow keypress during rapid navigation (no phantom second render, no reads until settled)", async () => {
		daemon.state = { snapshot: sixTaskSnapshot(), connected: true };
		let renders = 0;
		const app = render(
			<Profiler
				id="app"
				onRender={() => {
					renders += 1;
				}}
			>
				<App
					sockPath="/x.sock"
					runsDir="/runs"
					actions={noopActions}
					stdoutStream={big()}
				/>
			</Profiler>,
		);
		try {
			// Let mount settle: the lazy definitions() promise resolves and commits
			// once. Stay under the 120ms debounce so the initial selection's read has
			// not fired yet.
			await wait(40);
			const before = renders;
			const readsBefore = readRunFiles.mock.calls.length;

			// Five ↓ presses. Each moves the selection (one setState → one render) and
			// re-arms the run-files debounce (no read, no setRunFiles → no 2nd render).
			const N = 5;
			for (let i = 0; i < N; i += 1) {
				app.stdin.write(DOWN);
				await wait(10); // < 120ms cumulative (50ms) so the debounce never fires
			}
			const navRenders = renders - before;

			// The old code did 2 renders per keypress (selection + synchronous
			// setRunFiles). Assert we are at ~1 per keypress and well under 2N.
			expect(navRenders).toBeLessThan(2 * N);
			expect(navRenders).toBeLessThanOrEqual(N + 1);
			// Debounced: zero file reads while the selection is still moving.
			expect(readRunFiles.mock.calls.length).toBe(readsBefore);

			// Once the selection settles past the debounce, exactly one read fires.
			await wait(160);
			expect(readRunFiles.mock.calls.length).toBe(readsBefore + 1);
		} finally {
			app.unmount();
		}
	});

	it("run-files poll skips the re-render when content is unchanged and renders when it changes", async () => {
		daemon.state = { snapshot: sixTaskSnapshot(), connected: true };
		let renders = 0;
		const app = render(
			<Profiler
				id="app"
				onRender={() => {
					renders += 1;
				}}
			>
				<App
					sockPath="/x.sock"
					runsDir="/runs"
					actions={noopActions}
					stdoutStream={big()}
				/>
			</Profiler>,
		);
		try {
			// Settle mount + fire the debounced initial read (one setRunFiles render).
			// Generous headroom so the settle render lands *before* we sample
			// `before` — otherwise it would be miscounted against the poll window.
			await wait(400);
			const reads0 = readRunFiles.mock.calls.length;
			expect(reads0).toBeGreaterThanOrEqual(1);

			// Poll tick with identical content → read happens, but setRunFiles is
			// skipped (content-equal) so no render at all.
			const before = renders;
			await wait(1100);
			expect(readRunFiles.mock.calls.length).toBeGreaterThan(reads0);
			expect(renders).toBe(before);

			// Change the content → the next poll read differs → one render.
			runFilesContent.value = { report: "r1", transcriptTail: ["a", "b", "c"] };
			const beforeChange = renders;
			await wait(1100);
			expect(renders).toBe(beforeChange + 1);
		} finally {
			app.unmount();
		}
	});
});
