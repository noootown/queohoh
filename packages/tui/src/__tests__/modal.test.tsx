import { EventEmitter } from "node:events";
import { Box, Text } from "ink";
import { render } from "ink-testing-library";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { App } from "../App.js";
import { createActions } from "../actions.js";
import {
	Modal,
	modalGeometry,
	modalInnerWidth,
	padLine,
} from "../components/Modal.js";
import { TextInput } from "../components/TextInput.js";
import { cleanups, startServer } from "./helpers.js";

// Modal is absolute-positioned, so it only renders inside a sized
// `position="relative"` root (the real App usage). This mirrors that.
function renderInRoot(
	cols: number,
	rows: number,
	node: ReactNode,
	body?: ReactNode,
) {
	return render(
		<Box width={cols} height={rows} flexDirection="column" position="relative">
			{body}
			{node}
		</Box>,
	);
}

describe("modalGeometry", () => {
	it("clamps width to 72 on wide terminals and centers it", () => {
		const g = modalGeometry(200, 40, 4);
		expect(g.width).toBe(72);
		expect(g.marginLeft).toBe(64); // (200 - 72) / 2
	});

	it("tracks columns - 8 for mid-width terminals", () => {
		const g = modalGeometry(50, 40, 4);
		expect(g.width).toBe(42); // 50 - 8
		expect(g.marginLeft).toBe(4); // (50 - 42) / 2
	});

	it("floors width at 20 for very narrow terminals", () => {
		const g = modalGeometry(25, 40, 4);
		expect(g.width).toBe(20); // 25 - 8 = 17 -> floored to 20
		expect(g.marginLeft).toBe(2); // floor((25 - 20) / 2)
	});

	it("centers vertically from content height + border rows and floors", () => {
		const g = modalGeometry(80, 20, 4);
		// outer height = contentHeight(4) + border(2) = 6; (20 - 6) / 2 = 7
		expect(g.marginTop).toBe(7);
	});

	it("uses floor division for odd horizontal gaps", () => {
		const g = modalGeometry(41, 40, 4);
		expect(g.width).toBe(33); // 41 - 8
		expect(g.marginLeft).toBe(4); // floor((41 - 33) / 2) = 4
	});

	it("never returns negative offsets when the modal fills the screen", () => {
		const g = modalGeometry(20, 4, 10);
		expect(g.marginLeft).toBeGreaterThanOrEqual(0);
		expect(g.marginTop).toBeGreaterThanOrEqual(0);
	});
});

describe("padLine", () => {
	it("pads short text to the requested width with spaces", () => {
		expect(padLine("hi", 6)).toBe("hi    ");
		expect(padLine("hi", 6)).toHaveLength(6);
	});

	it("returns text unchanged when already at width", () => {
		expect(padLine("abcdef", 6)).toBe("abcdef");
	});

	it("truncates text longer than width", () => {
		expect(padLine("abcdefgh", 6)).toBe("abcdef");
	});
});

describe("modalInnerWidth", () => {
	it("subtracts the two border columns (padding baked into text)", () => {
		expect(modalInnerWidth(32)).toBe(30);
	});
});

describe("Modal", () => {
	it("renders the title and dim hint line", () => {
		const { lastFrame } = renderInRoot(
			60,
			20,
			<Modal title="Add task" columns={60} rows={20} hint="esc close">
				<Text>body</Text>
			</Modal>,
		);
		const frame = lastFrame() ?? "";
		expect(frame).toContain("Add task");
		expect(frame).toContain("esc close");
	});

	it("renders children inside the modal", () => {
		const { lastFrame } = renderInRoot(
			60,
			20,
			<Modal title="Pick" columns={60} rows={20} hint="esc">
				<Text>option-one</Text>
			</Modal>,
		);
		expect(lastFrame() ?? "").toContain("option-one");
	});

	it("pads the title line to the full inner width so it is opaque", () => {
		const { lastFrame } = renderInRoot(
			40,
			20,
			<Modal title="Hi" columns={40} rows={20} hint="esc">
				<Text>x</Text>
			</Modal>,
		);
		const frame = lastFrame() ?? "";
		const titleRow = frame.split("\n").find((l) => l.includes("Hi")) ?? "";
		// width = 40 - 8 = 32; inner = 28. "Hi" is followed by a run of spaces
		// (then padding + right border), so trailing space is preserved.
		expect(titleRow).toMatch(/Hi\s{20,}/);
	});

	it("composites over body text: body outside visible, interior opaque", () => {
		const cols = 60;
		const rows = 16;
		const fill = "X".repeat(cols);
		const body = Array.from({ length: rows }, (_, i) => (
			// biome-ignore lint/suspicious/noArrayIndexKey: static fixture
			<Text key={i}>{fill}</Text>
		));
		const inner = modalInnerWidth(modalGeometry(cols, rows, 3).width);
		const { lastFrame } = renderInRoot(
			cols,
			rows,
			<Modal title="TITLE" columns={cols} rows={rows} hint="esc close">
				<Text>{padLine("content-line", inner)}</Text>
			</Modal>,
			body,
		);
		const frame = lastFrame() ?? "";
		const lines = frame.split("\n");
		// (c) top body row untouched.
		expect(lines[0]).toContain("XXXXXXXXXX");
		// (a) a modal border corner appears (absolute offset applied).
		expect(frame).toMatch(/[╭╮╰╯]/);
		// (b) interior content row overwrites body: no X between the content text
		// and the right border (self-padded child is fully opaque).
		const contentRow = lines.find((l) => l.includes("content-line")) ?? "";
		const idx = contentRow.indexOf("content-line");
		const rightBorder = contentRow.indexOf("│", idx);
		const between = contentRow.slice(idx + "content-line".length, rightBorder);
		expect(between).not.toContain("X");
		// (d) the title and hint rows are equally opaque: no body X survives
		// between the left and right borders of either row.
		for (const marker of ["TITLE", "esc close"]) {
			const row = lines.find((l) => l.includes(marker)) ?? "";
			const left = row.indexOf("│");
			const right = row.indexOf("│", left + 1);
			expect(left).toBeGreaterThanOrEqual(0);
			expect(right).toBeGreaterThan(left);
			expect(row.slice(left + 1, right)).not.toContain("X");
		}
		// (e) compositing leaves the body visible outside the modal: X survives to
		// both the LEFT and RIGHT of the modal on a modal-spanned row.
		const spannedRow = lines.find((l) => l.includes("TITLE")) ?? "";
		const leftBorder = spannedRow.indexOf("│");
		const rightBorderEdge = spannedRow.lastIndexOf("│");
		expect(spannedRow.slice(0, leftBorder)).toContain("X");
		expect(spannedRow.slice(rightBorderEdge + 1)).toContain("X");
	});
});

describe("TextInput composed inside Modal", () => {
	it("is opaque over body text when given the modal inner width", () => {
		const cols = 60;
		const rows = 16;
		const fill = "X".repeat(cols);
		const body = Array.from({ length: rows }, (_, i) => (
			// biome-ignore lint/suspicious/noArrayIndexKey: static fixture
			<Text key={i}>{fill}</Text>
		));
		const inner = modalInnerWidth(modalGeometry(cols, rows, 3).width);
		const { lastFrame } = renderInRoot(
			cols,
			rows,
			<Modal title="Add" columns={cols} rows={rows} hint="esc close">
				<TextInput
					label="prompt"
					value="hi"
					width={inner}
					onChange={() => {}}
					onSubmit={() => {}}
					onCancel={() => {}}
				/>
			</Modal>,
			body,
		);
		const frame = lastFrame() ?? "";
		const line = frame.split("\n").find((l) => l.includes("prompt")) ?? "";
		expect(line).toContain("prompt> hi");
		const idx = line.indexOf("prompt");
		const rightBorder = line.indexOf("│", idx);
		const between = line.slice(idx, rightBorder);
		expect(between).not.toContain("X");
	});
});

// --- App-driven action menu -------------------------------------------------
// These exercise the real App wiring (daemon snapshot + createActions), driving
// keys through app.stdin exactly like app.test.tsx, then asserting on frames and
// on daemon/store side effects.

afterEach(async () => {
	while (cleanups.length) await cleanups.pop()?.();
});

const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

const CTRL_S = "\u0013";
const DOWN = "\u001b[B";
const ESC = "\u001b";

type FakeStream = EventEmitter & { columns: number; rows: number };
function fakeStream(columns: number, rows: number): FakeStream {
	const emitter = new EventEmitter() as FakeStream;
	emitter.columns = columns;
	emitter.rows = rows;
	return emitter;
}
const big = () => fakeStream(120, 40) as unknown as NodeJS.WriteStream;

// Move focus from the default queue pane down to the worktrees pane.
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

describe("action menu", () => {
	it("a on a failed queue row opens the menu with Rerun enabled", async () => {
		const { store, server, sock, base } = await startServer();
		// The summary tail (TITLETAIL) is chosen to be visible ONLY in the modal
		// title: the queue pane column is narrow enough to truncate it away, while
		// promptSummary keeps the full 50-char line and the modal title (~70 inner
		// cols) renders it whole. So the tail assertion locks in that the menu
		// title is the targeted item's name, not the background pane row.
		const task = store.create({
			prompt: "boom task xxxxxxxxxxxxxxxxxxxxxxxxxxxxxx TITLETAIL",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		store.update(task.id, { status: "failed", error: "boom" });
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
		// self-validation: before the menu opens, the tail is nowhere on screen
		// (the queue pane truncates it), so seeing it later proves it came from
		// the modal title.
		expect(app.lastFrame() ?? "").not.toContain("TITLETAIL");
		app.stdin.write("a");
		await wait(60);
		const frame = app.lastFrame() ?? "";
		// menu-open sentinel: the action-menu hint (unique to this modal); the
		// modal title is the targeted item's name (the queue row summary).
		expect(frame).toContain("enter run · esc close");
		expect(frame).toContain("TITLETAIL");
		expect(frame).toContain("Rerun");
		expect(frame).toContain("Skip");
		expect(frame).toContain("Assign worktree…");
		// Assign worktree… is disabled for a failed task and shows its reason.
		expect(frame).toContain("only for needs-input tasks");
		// Rerun is enabled for a failed task: its row carries no disabled reason
		// (disabled rows render with an em-dash separator).
		const rerunLine = frame.split("\n").find((l) => l.includes("Rerun")) ?? "";
		expect(rerunLine).not.toContain("—");
	});

	it("enter on an enabled row executes and closes the menu (retry)", async () => {
		const { store, server, sock, base } = await startServer();
		const task = store.create({
			prompt: "boom task",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		store.update(task.id, { status: "failed", error: "boom" });
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
		app.stdin.write("a");
		await wait(60);
		expect(app.lastFrame()).toContain("enter run · esc close");
		app.stdin.write("\r"); // enter on Rerun (index 0, enabled)
		await wait(150);
		// menu closed …
		expect(app.lastFrame()).not.toContain("enter run · esc close");
		// … and retry flipped the task back to queued in the shared store.
		expect(store.get(task.id)?.status).toBe("queued");
	});

	it("enter on a disabled row does nothing", async () => {
		const { store, server, sock, base } = await startServer();
		const task = store.create({
			prompt: "busy task",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		store.update(task.id, { status: "running" });
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
		app.stdin.write("a");
		await wait(60);
		expect(app.lastFrame()).toContain("enter run · esc close");
		// Rerun is disabled for a running task and shows its reason.
		const rerunLine =
			(app.lastFrame() ?? "").split("\n").find((l) => l.includes("Rerun")) ??
			"";
		expect(rerunLine).toContain("cannot rerun a running task");
		app.stdin.write("\r"); // enter on the disabled Rerun row
		await wait(150);
		// menu stays open, status unchanged (no retry fired).
		expect(app.lastFrame()).toContain("enter run · esc close");
		expect(store.get(task.id)?.status).toBe("running");
	});

	it("worktree menu: Remove worktree… opens y/n confirm; y calls removeWorktree", async () => {
		const execCalls: { command: string; args: string[] }[] = [];
		const { engine, server, sock, base } = await startServer({
			worktrees: [{ name: "wt-a", path: "/wt/wt-a", branch: "wt-a" }],
			execCalls,
		});
		await engine.tick();
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
		expect(app.lastFrame()).toContain("wt-a");
		await focusWorktrees(app);
		app.stdin.write("a");
		await wait(60);
		expect(app.lastFrame()).toContain("enter run · esc close");
		expect(app.lastFrame()).toContain("Remove worktree…");
		// Remove worktree… is the 5th row (index 4): j×4.
		for (let i = 0; i < 4; i += 1) {
			app.stdin.write("j");
			await wait(20);
		}
		app.stdin.write("\r"); // enter → confirm-remove modal
		await wait(60);
		expect(app.lastFrame()).toContain("Remove worktree — wt-a");
		expect(app.lastFrame()).toContain("discards uncommitted changes");
		expect(app.lastFrame()).toContain("deletes the local branch");
		expect(app.lastFrame()).toContain("y confirm");
		app.stdin.write("y"); // confirm
		await wait(150);
		// daemon ran `wt remove <branch> --yes` (exec recorded by the fixture).
		expect(execCalls).toContainEqual({
			command: "wt",
			args: ["remove", "wt-a", "--yes"],
		});
	});

	it("esc closes the menu without acting", async () => {
		const { store, server, sock, base } = await startServer();
		const task = store.create({
			prompt: "boom task",
			repo: "platform",
			ref: "temp",
			source: "tui",
		});
		store.update(task.id, { status: "failed", error: "boom" });
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
		app.stdin.write("a");
		await wait(60);
		expect(app.lastFrame()).toContain("enter run · esc close");
		app.stdin.write(ESC);
		await wait(60);
		expect(app.lastFrame()).not.toContain("enter run · esc close");
		expect(store.get(task.id)?.status).toBe("failed");
	});
});
