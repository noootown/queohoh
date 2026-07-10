import type { ArgSpec, TaskDefinition } from "@queohoh/core";
import { Text } from "ink";
import { render } from "ink-testing-library";
import { describe, expect, it } from "vitest";
import type { DefinitionSummary } from "../actions.js";
import { ArgsForm } from "../components/ArgsForm.js";
import { DetailPane } from "../components/DetailPane.js";
import { Footer } from "../components/Footer.js";
import { Pane } from "../components/Pane.js";
import { QueuePane } from "../components/QueuePanel.js";
import { TabBar } from "../components/TabBar.js";
import { TasksPane } from "../components/TasksPane.js";
import { TextInput } from "../components/TextInput.js";
import { WorktreesPane } from "../components/WorktreesPane.js";
import type { QueueRow } from "../format.js";
import type { ProjectTab, WorktreeRow } from "../selectors.js";
import { makeTask } from "./helpers.js";

const rows: QueueRow[] = [
	{
		id: "01A",
		glyph: "▶",
		sessionMarker: "",
		lane: "platform:JUS-1",
		summary: "reply to review",
		detail: "⏱ 3m12s",
		kind: "live",
	},
	{
		id: "01B",
		glyph: "○",
		sessionMarker: "",
		lane: "platform:JUS-1",
		summary: "fix flaky test",
		detail: "#1 in lane",
		kind: "live",
	},
];

function queueRow(id: string, summary: string): QueueRow {
	return {
		id,
		glyph: "○",
		sessionMarker: "",
		lane: "platform:JUS-1",
		summary,
		detail: "#1 in lane",
		kind: "live",
	};
}

describe("TabBar", () => {
	const tabs: ProjectTab[] = [
		{ name: "platform", synthetic: false },
		{ name: "queohoh", synthetic: true },
	];

	it("lists numbered tabs, active highlight, connected + running", () => {
		const { lastFrame } = render(
			<TabBar
				tabs={tabs}
				activeIndex={0}
				connected={true}
				runningCount={1}
				maxConcurrent={2}
			/>,
		);
		expect(lastFrame()).toContain("1:platform");
		expect(lastFrame()).toContain("2:queohoh");
		expect(lastFrame()).toContain("running 1/2");
	});

	it("shows the unreachable banner when disconnected", () => {
		const { lastFrame } = render(
			<TabBar
				tabs={tabs}
				activeIndex={0}
				connected={false}
				runningCount={0}
				maxConcurrent={2}
			/>,
		);
		expect(lastFrame()).toContain("daemon unreachable");
	});

	it("omits /N when maxConcurrent is null", () => {
		const { lastFrame } = render(
			<TabBar
				tabs={tabs}
				activeIndex={0}
				connected={true}
				runningCount={3}
				maxConcurrent={null}
			/>,
		);
		expect(lastFrame()).toContain("running 3");
		expect(lastFrame()).not.toContain("running 3/");
	});
});

describe("Pane", () => {
	it("renders a bold title and its children", () => {
		const { lastFrame } = render(
			<Pane title="QUEUE" focused={false}>
				<Text>body text</Text>
			</Pane>,
		);
		expect(lastFrame()).toContain("QUEUE");
		expect(lastFrame()).toContain("body text");
	});

	// Border color (cyan vs gray) is driven by the `focused` prop, but
	// ink-testing-library strips ANSI so the two frames are byte-identical here.
	// Assert the structural intent instead: a rounded border + title render in
	// both focus states without throwing.
	it("renders a rounded border and title in both focus states", () => {
		for (const focused of [true, false]) {
			const { lastFrame } = render(
				<Pane title="QUEUE" focused={focused}>
					<Text>body</Text>
				</Pane>,
			);
			expect(lastFrame()).toContain("╭");
			expect(lastFrame()).toContain("QUEUE");
			expect(lastFrame()).toContain("body");
		}
	});
});

describe("QueuePane", () => {
	it("renders rows with glyphs and details", () => {
		const { lastFrame } = render(
			<QueuePane
				rows={rows}
				selection={{ cursor: 0, anchor: null }}
				focused={true}
				capacity={10}
				filter=""
				filterActive={false}
			/>,
		);
		expect(lastFrame()).toContain("QUEUE");
		expect(lastFrame()).toContain("▶");
		expect(lastFrame()).toContain("reply to review");
		expect(lastFrame()).toContain("#1 in lane");
	});

	it("renders the empty state", () => {
		const { lastFrame } = render(
			<QueuePane
				rows={[]}
				selection={{ cursor: 0, anchor: null }}
				focused={true}
				capacity={10}
				filter=""
				filterActive={false}
			/>,
		);
		expect(lastFrame()).toContain(
			"queue empty — [a] on a worktree to add a task",
		);
	});

	// Archived rows carry dimColor (stripped by the test harness), so the visible
	// intent we can pin is that both live and archived rows still render.
	it("renders both live and archived rows", () => {
		const live = render(
			<QueuePane
				rows={[queueRow("L", "live row text")]}
				selection={{ cursor: -1, anchor: null }}
				focused={false}
				capacity={10}
				filter=""
				filterActive={false}
			/>,
		).lastFrame();
		const archived = render(
			<QueuePane
				rows={[{ ...queueRow("A", "archived row text"), kind: "archived" }]}
				selection={{ cursor: -1, anchor: null }}
				focused={false}
				capacity={10}
				filter=""
				filterActive={false}
			/>,
		).lastFrame();
		expect(live).toContain("live row text");
		expect(archived).toContain("archived row text");
	});

	it("renders the chain marker for main-session rows", () => {
		const { lastFrame } = render(
			<QueuePane
				rows={[{ ...queueRow("M", "main row"), sessionMarker: "⛓ " }]}
				selection={{ cursor: -1, anchor: null }}
				focused={false}
				capacity={10}
				filter=""
				filterActive={false}
			/>,
		);
		expect(lastFrame()).toContain("⛓");
		expect(lastFrame()).toContain("main row");
	});

	it("windows to capacity keeping the selected row visible", () => {
		const many = Array.from({ length: 5 }, (_, i) =>
			queueRow(`R${i}`, `row-${i}`),
		);
		const { lastFrame } = render(
			<QueuePane
				rows={many}
				selection={{ cursor: 4, anchor: null }}
				focused={true}
				capacity={2}
				filter=""
				filterActive={false}
			/>,
		);
		expect(lastFrame()).toContain("row-4");
		expect(lastFrame()).not.toContain("row-0");
	});
});

describe("TasksPane", () => {
	const defs: DefinitionSummary[] = [
		{
			repo: "platform",
			name: "review",
			scope: "project",
			args: [],
			hasDiscovery: true,
		},
		{
			repo: "platform",
			name: "ticket",
			scope: "project",
			args: [{ name: "id" }, { name: "flag" }],
			hasDiscovery: false,
		},
	];

	it("renders name, args and discovery badge", () => {
		const { lastFrame } = render(
			<TasksPane
				defs={defs}
				selection={{ cursor: 0, anchor: null }}
				focused={true}
				capacity={10}
				filter=""
				filterActive={false}
			/>,
		);
		expect(lastFrame()).toContain("review");
		expect(lastFrame()).toContain("⏰");
		expect(lastFrame()).toContain("(id, flag)");
	});
});

describe("WorktreesPane", () => {
	const wtRows: WorktreeRow[] = [
		{
			kind: "worktree",
			name: "wt-a",
			path: "/wt/a",
			branch: "a",
			state: "busy",
			hasMainSession: false,
			queued: 2,
		},
		{
			kind: "session",
			name: "platform",
			path: "/ws/platform",
			branch: null,
			state: "you",
			hasMainSession: false,
			queued: 0,
		},
	];

	it("renders a colored-dot prefix, the name, and a queued-count badge (no state word)", () => {
		const { lastFrame } = render(
			<WorktreesPane
				rows={wtRows}
				selection={{ cursor: 0, anchor: null }}
				focused={true}
				capacity={10}
				filter=""
				filterActive={false}
			/>,
		);
		const frame = lastFrame() ?? "";
		expect(frame).toContain("wt-a");
		expect(frame).toContain("platform");
		// compact dot prefix replaces the old "busy"/"free"/"YOU" words
		expect(frame).toContain("●");
		expect(frame).not.toContain("busy");
		expect(frame).not.toContain("YOU");
		// queued badge shown only when > 0
		expect(frame).toContain("[2]");
	});

	// A wrapped row consumes extra terminal lines beyond the row capacity,
	// overflowing the fixed-height pane and pushing the title out of view.
	// Every list row must truncate to exactly one line.
	it("truncates long names to a single line instead of wrapping", () => {
		const long: WorktreeRow = {
			kind: "worktree",
			name: "w".repeat(300),
			path: "/wt/long",
			branch: "long",
			state: "free",
			hasMainSession: false,
			queued: 0,
		};
		const { lastFrame } = render(
			<WorktreesPane
				rows={[long]}
				selection={{ cursor: 0, anchor: null }}
				focused={false}
				capacity={10}
				filter=""
				filterActive={false}
			/>,
		);
		const frame = lastFrame() ?? "";
		expect(frame).toContain("WORKTREES");
		const nameLines = frame.split("\n").filter((l) => l.includes("www"));
		expect(nameLines).toHaveLength(1);
	});
});

describe("WorktreesPane — range selection", () => {
	const rows = [
		{
			kind: "worktree" as const,
			name: "wt-a",
			path: "/wt/wt-a",
			branch: "wt-a",
			state: "free" as const,
			hasMainSession: false,
			queued: 0,
		},
		{
			kind: "worktree" as const,
			name: "wt-b",
			path: "/wt/wt-b",
			branch: "wt-b",
			state: "free" as const,
			hasMainSession: false,
			queued: 0,
		},
		{
			kind: "worktree" as const,
			name: "wt-c",
			path: "/wt/wt-c",
			branch: "wt-c",
			state: "free" as const,
			hasMainSession: false,
			queued: 0,
		},
	];

	it("renders every row in the range inverse and counts them in the title", () => {
		const { lastFrame } = render(
			<WorktreesPane
				rows={rows}
				selection={{ cursor: 1, anchor: 0 }}
				focused={true}
				capacity={5}
				filter=""
				filterActive={false}
			/>,
		);
		const frame = lastFrame() ?? "";
		expect(frame).toContain("· 2 selected");
		// both range rows carry the inverse escape, the third does not
		expect(frame).toMatch(/\[7m[^\n]*wt-a/);
		expect(frame).toMatch(/\[7m[^\n]*wt-b/);
		expect(frame).not.toMatch(/\[7m[^\n]*wt-c/);
	});

	it("single selection renders exactly one inverse row and no count", () => {
		const { lastFrame } = render(
			<WorktreesPane
				rows={rows}
				selection={{ cursor: 1, anchor: null }}
				focused={true}
				capacity={5}
				filter=""
				filterActive={false}
			/>,
		);
		const frame = lastFrame() ?? "";
		expect(frame).not.toContain("selected");
		expect(frame).not.toMatch(/\[7m[^\n]*wt-a/);
		expect(frame).toMatch(/\[7m[^\n]*wt-b/);
	});
});

describe("list rows never wrap (title stays visible)", () => {
	it("QueuePane truncates long summaries to a single line", () => {
		const { lastFrame } = render(
			<QueuePane
				rows={[queueRow("L1", "s".repeat(300))]}
				selection={{ cursor: 0, anchor: null }}
				focused={false}
				capacity={10}
				filter=""
				filterActive={false}
			/>,
		);
		const frame = lastFrame() ?? "";
		expect(frame).toContain("QUEUE");
		expect(frame.split("\n").filter((l) => l.includes("sss"))).toHaveLength(1);
	});

	it("TasksPane truncates long task names to a single line", () => {
		const defs: DefinitionSummary[] = [
			{
				repo: "platform",
				name: "t".repeat(300),
				scope: "project",
				args: [],
				hasDiscovery: false,
			},
		];
		const { lastFrame } = render(
			<TasksPane
				defs={defs}
				selection={{ cursor: 0, anchor: null }}
				focused={false}
				capacity={10}
				filter=""
				filterActive={false}
			/>,
		);
		const frame = lastFrame() ?? "";
		expect(frame).toContain("TASKS");
		expect(frame.split("\n").filter((l) => l.includes("ttt"))).toHaveLength(1);
	});

	it("renders a chain marker before state when hasMainSession", () => {
		const withMain: WorktreeRow[] = [
			{
				kind: "worktree",
				name: "wt-x",
				path: "/wt/x",
				branch: "x",
				state: "free",
				hasMainSession: true,
				queued: 0,
			},
		];
		const { lastFrame } = render(
			<WorktreesPane
				rows={withMain}
				selection={{ cursor: -1, anchor: null }}
				focused={false}
				capacity={10}
				filter=""
				filterActive={false}
			/>,
		);
		expect(lastFrame()).toContain("◆");
	});
});

describe("Footer", () => {
	it("prefers the status line (red) over hints", () => {
		const { lastFrame } = render(
			<Footer
				focus="queue"
				prefixArmed={false}
				statusLine="boom"
				searching={false}
				selectionCount={0}
			/>,
		);
		expect(lastFrame()).toContain("boom");
		expect(lastFrame()).not.toContain("[a]dd");
	});

	it("shows the prefix indicator when armed", () => {
		const { lastFrame } = render(
			<Footer
				focus="queue"
				prefixArmed={true}
				statusLine={null}
				searching={false}
				selectionCount={0}
			/>,
		);
		expect(lastFrame()).toContain("PREFIX");
		expect(lastFrame()).toContain("n/p cycle");
	});

	it("shows queue hints without [a]dd", () => {
		const { lastFrame } = render(
			<Footer
				focus="queue"
				prefixArmed={false}
				statusLine={null}
				searching={false}
				selectionCount={0}
			/>,
		);
		expect(lastFrame()).not.toContain("[a]dd");
		expect(lastFrame()).toContain("[enter] detail");
		expect(lastFrame()).toContain("[q]uit");
	});

	it("shows tasks hints (actions moved into the [a] menu)", () => {
		const { lastFrame } = render(
			<Footer
				focus="tasks"
				prefixArmed={false}
				statusLine={null}
				searching={false}
				selectionCount={0}
			/>,
		);
		expect(lastFrame()).toContain("[a] actions");
		expect(lastFrame()).toContain("[enter] detail");
		expect(lastFrame()).not.toContain("[enter] run");
	});

	it("shows worktrees hints (fresh/main/run-def moved into the [a] menu)", () => {
		const { lastFrame } = render(
			<Footer
				focus="worktrees"
				prefixArmed={false}
				statusLine={null}
				searching={false}
				selectionCount={0}
			/>,
		);
		expect(lastFrame()).toContain("[a] actions");
		expect(lastFrame()).toContain("[enter] detail");
		expect(lastFrame()).not.toContain("[f]resh task");
		expect(lastFrame()).not.toContain("[m]ain task");
	});

	it("shows detail hints", () => {
		const { lastFrame } = render(
			<Footer
				focus="detail"
				prefixArmed={false}
				statusLine={null}
				searching={false}
				selectionCount={0}
			/>,
		);
		expect(lastFrame()).toContain("sub-tab");
		expect(lastFrame()).toContain("[g/G] top/bottom");
	});

	it("shows the search hint while searching, overriding other hints", () => {
		const { lastFrame } = render(
			<Footer
				focus="queue"
				prefixArmed={true}
				statusLine="boom"
				searching={true}
				selectionCount={0}
			/>,
		);
		expect(lastFrame()).toContain("type to filter");
		expect(lastFrame()).toContain("[enter] apply");
		expect(lastFrame()).toContain("[esc] clear");
		expect(lastFrame()).not.toContain("boom");
		expect(lastFrame()).not.toContain("PREFIX");
	});
});

describe("TextInput", () => {
	it("appends typed chars and submits on enter", async () => {
		let submitted = "";
		let value = "";
		const { stdin, rerender, lastFrame } = render(
			<TextInput
				label="prompt"
				value={value}
				onChange={(v) => {
					value = v;
				}}
				onSubmit={(v) => {
					submitted = v;
				}}
				onCancel={() => {}}
			/>,
		);
		stdin.write("h");
		rerender(
			<TextInput
				label="prompt"
				value={value}
				onChange={(v) => {
					value = v;
				}}
				onSubmit={(v) => {
					submitted = v;
				}}
				onCancel={() => {}}
			/>,
		);
		stdin.write("i");
		rerender(
			<TextInput
				label="prompt"
				value={value}
				onChange={(v) => {
					value = v;
				}}
				onSubmit={(v) => {
					submitted = v;
				}}
				onCancel={() => {}}
			/>,
		);
		expect(lastFrame()).toContain("prompt> hi");
		stdin.write("\r");
		expect(submitted).toBe("hi");
	});
});

function makeDefinition(
	overrides: Partial<TaskDefinition> = {},
): TaskDefinition {
	return {
		name: "review",
		repo: "platform",
		discovery: { command: "gh pr list", itemKey: "number" },
		args: [{ name: "id" }, { name: "flag" }],
		dedup: "skip_seen",
		worktree: "temp",
		preRun: null,
		postRun: null,
		model: "sonnet",
		timeoutMs: 1800000,
		priority: "normal",
		prompt: "line one\nline two\nline three\n",
		...overrides,
	};
}

const noFiles = { report: null, transcriptTail: [] as string[] };

describe("DetailPane", () => {
	it("renders (nothing selected) for the empty context", () => {
		const { lastFrame } = render(
			<DetailPane
				context={{ kind: "empty" }}
				subTab={0}
				focused={false}
				width={40}
				height={10}
				scrollOffset={0}
				runFiles={null}
				definition={null}
			/>,
		);
		expect(lastFrame()).toContain("(nothing selected)");
	});

	it("renders the sub-tab strip for a run context", () => {
		const { lastFrame } = render(
			<DetailPane
				context={{ kind: "run", task: makeTask("running") }}
				subTab={0}
				focused={true}
				width={40}
				height={10}
				scrollOffset={0}
				runFiles={{ report: null, transcriptTail: ["hello"] }}
				definition={null}
			/>,
		);
		expect(lastFrame()).toContain("1:transcript");
		expect(lastFrame()).toContain("2:report");
		expect(lastFrame()).toContain("3:prompt");
	});

	it("run/transcript renders the tail and its placeholder", () => {
		const withTail = render(
			<DetailPane
				context={{ kind: "run", task: makeTask("running") }}
				subTab={0}
				focused={true}
				width={40}
				height={10}
				scrollOffset={0}
				runFiles={{ report: null, transcriptTail: ["first", "second"] }}
				definition={null}
			/>,
		).lastFrame();
		expect(withTail).toContain("first");
		expect(withTail).toContain("second");

		const empty = render(
			<DetailPane
				context={{ kind: "run", task: makeTask("running") }}
				subTab={0}
				focused={true}
				width={40}
				height={10}
				scrollOffset={0}
				runFiles={noFiles}
				definition={null}
			/>,
		).lastFrame();
		expect(empty).toContain("(no transcript yet)");
	});

	it("run/transcript windows to height showing the newest lines", () => {
		const tail = ["l0", "l1", "l2", "l3", "l4", "l5"];
		const { lastFrame } = render(
			<DetailPane
				context={{ kind: "run", task: makeTask("running") }}
				subTab={0}
				focused={true}
				width={40}
				height={2}
				scrollOffset={0}
				runFiles={{ report: null, transcriptTail: tail }}
				definition={null}
			/>,
		);
		expect(lastFrame()).toContain("l5");
		expect(lastFrame()).not.toContain("l0");
	});

	it("run/report renders report text and its placeholder", () => {
		const withReport = render(
			<DetailPane
				context={{ kind: "run", task: makeTask("done") }}
				subTab={1}
				focused={true}
				width={40}
				height={10}
				scrollOffset={0}
				runFiles={{ report: "all green here", transcriptTail: [] }}
				definition={null}
			/>,
		).lastFrame();
		expect(withReport).toContain("all green here");

		const empty = render(
			<DetailPane
				context={{ kind: "run", task: makeTask("done") }}
				subTab={1}
				focused={true}
				width={40}
				height={10}
				scrollOffset={0}
				runFiles={noFiles}
				definition={null}
			/>,
		).lastFrame();
		expect(empty).toContain("(no report yet)");
	});

	it("run/prompt renders the task prompt", () => {
		const { lastFrame } = render(
			<DetailPane
				context={{
					kind: "run",
					task: makeTask("running", { prompt: "carry out the plan\n" }),
				}}
				subTab={2}
				focused={true}
				width={40}
				height={10}
				scrollOffset={0}
				runFiles={noFiles}
				definition={null}
			/>,
		);
		expect(lastFrame()).toContain("carry out the plan");
	});

	it("definition/prompt renders the definition prompt or loading", () => {
		const loaded = render(
			<DetailPane
				context={{ kind: "definition", repo: "platform", name: "review" }}
				subTab={0}
				focused={true}
				width={40}
				height={10}
				scrollOffset={0}
				runFiles={null}
				definition={makeDefinition()}
			/>,
		).lastFrame();
		expect(loaded).toContain("line one");

		const loading = render(
			<DetailPane
				context={{ kind: "definition", repo: "platform", name: "review" }}
				subTab={0}
				focused={true}
				width={40}
				height={10}
				scrollOffset={0}
				runFiles={null}
				definition={null}
			/>,
		).lastFrame();
		expect(loading).toContain("(loading definition…)");
	});

	it("definition/config renders one line per field", () => {
		const { lastFrame } = render(
			<DetailPane
				context={{ kind: "definition", repo: "platform", name: "review" }}
				subTab={1}
				focused={true}
				width={60}
				height={20}
				scrollOffset={0}
				runFiles={null}
				definition={makeDefinition()}
			/>,
		);
		const frame = lastFrame() ?? "";
		expect(frame).toContain("args: id, flag");
		expect(frame).toContain("worktree: temp");
		expect(frame).toContain("dedup: skip_seen");
		expect(frame).toContain("model: sonnet");
		expect(frame).toContain("1800000ms");
		expect(frame).toContain("priority: normal");
		expect(frame).toContain("gh pr list");
	});

	it("definition/config renders — for absent discovery", () => {
		const { lastFrame } = render(
			<DetailPane
				context={{ kind: "definition", repo: "platform", name: "review" }}
				subTab={1}
				focused={true}
				width={60}
				height={20}
				scrollOffset={0}
				runFiles={null}
				definition={makeDefinition({ discovery: null })}
			/>,
		);
		expect(lastFrame()).toContain("discovery: —");
	});

	it("worktree/info renders path, branch, state and lane tasks", () => {
		const row: WorktreeRow = {
			kind: "worktree",
			name: "wt-a",
			path: "/wt/a",
			branch: "feature-a",
			state: "busy",
			hasMainSession: false,
			queued: 0,
		};
		const { lastFrame } = render(
			<DetailPane
				context={{
					kind: "worktree",
					row,
					laneTasks: [makeTask("running", { prompt: "fix the flaky test\n" })],
				}}
				subTab={0}
				focused={true}
				width={40}
				height={20}
				scrollOffset={0}
				runFiles={null}
				definition={null}
			/>,
		);
		const frame = lastFrame() ?? "";
		expect(frame).toContain("/wt/a");
		expect(frame).toContain("feature-a");
		expect(frame).toContain("busy");
		expect(frame).toContain("tasks on this lane:");
		expect(frame).toContain("fix the flaky test");
	});

	it("worktree/info renders (none) with no lane tasks", () => {
		const row: WorktreeRow = {
			kind: "worktree",
			name: "wt-a",
			path: "/wt/a",
			branch: null,
			state: "free",
			hasMainSession: false,
			queued: 0,
		};
		const { lastFrame } = render(
			<DetailPane
				context={{ kind: "worktree", row, laneTasks: [] }}
				subTab={0}
				focused={true}
				width={40}
				height={20}
				scrollOffset={0}
				runFiles={null}
				definition={null}
			/>,
		);
		expect(lastFrame()).toContain("(none)");
	});
});

describe("ArgsForm", () => {
	const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));
	const TAB = "\t";
	const SHIFT_TAB = "[Z";
	const LEFT = "[D";
	const RIGHT = "[C";
	const ESC = "";

	const renderForm = (
		args: ArgSpec[],
		opts: {
			initial?: Record<string, string>;
			fixed?: Record<string, string>;
			onSubmit?: (v: string[]) => void;
			onCancel?: () => void;
		} = {},
	) =>
		render(
			<ArgsForm
				args={args}
				initial={opts.initial}
				fixed={opts.fixed}
				width={60}
				onSubmit={opts.onSubmit ?? (() => {})}
				onCancel={opts.onCancel ?? (() => {})}
			/>,
		);

	it("prefills text args with their default and shows enum brackets", async () => {
		const { lastFrame } = renderForm([
			{ name: "pr", description: "PR number" },
			{ name: "mode", default: "ready", options: ["ready", "create"] },
			{ name: "review", default: "auto" },
		]);
		await wait(20);
		const frame = lastFrame() ?? "";
		expect(frame).toContain("pr>");
		expect(frame).toContain("mode>");
		expect(frame).toContain("‹ready›"); // enum shows default in brackets
		expect(frame).toContain("auto"); // text arg prefilled with its default
		expect(frame).toContain("PR number"); // description dimmed to the right
	});

	it("cycles enum options with →/← and ignores typing on enum rows", async () => {
		let submitted: string[] | null = null;
		const { lastFrame, stdin } = renderForm(
			[{ name: "mode", default: "ready", options: ["ready", "create"] }],
			{ onSubmit: (v) => (submitted = v) },
		);
		await wait(20);
		stdin.write("x"); // typing ignored on an enum row
		await wait(20);
		expect(lastFrame() ?? "").toContain("‹ready›");
		stdin.write(RIGHT); // ready -> create
		await wait(20);
		expect(lastFrame() ?? "").toContain("‹create›");
		stdin.write(RIGHT); // create -> wraps to ready
		await wait(20);
		expect(lastFrame() ?? "").toContain("‹ready›");
		stdin.write(LEFT); // ready -> wraps back to create
		await wait(20);
		expect(lastFrame() ?? "").toContain("‹create›");
		stdin.write("\r");
		await wait(20);
		expect(submitted).toEqual(["create"]);
	});

	it("tab/shift-tab move focus, wrapping, and submit is positional in arg order", async () => {
		let submitted: string[] | null = null;
		const { stdin } = renderForm(
			[{ name: "a" }, { name: "b" }, { name: "c" }],
			{ onSubmit: (v) => (submitted = v) },
		);
		await wait(20);
		stdin.write("1"); // -> a
		stdin.write(TAB);
		stdin.write("2"); // -> b
		stdin.write(TAB);
		stdin.write("3"); // -> c
		stdin.write(TAB); // wraps back to a
		stdin.write("4"); // a becomes "14"
		await wait(30);
		stdin.write(SHIFT_TAB); // wraps a -> c
		stdin.write("9"); // c becomes "39"
		await wait(30);
		stdin.write("\r");
		await wait(20);
		expect(submitted).toEqual(["14", "2", "39"]);
	});

	it("blocks submit on a required-empty field with an inline error", async () => {
		let submitted: string[] | null = null;
		const { lastFrame, stdin } = renderForm([{ name: "pr" }], {
			onSubmit: (v) => (submitted = v),
		});
		await wait(20);
		stdin.write("\r"); // required + empty -> blocked
		await wait(20);
		expect(submitted).toBeNull();
		expect(lastFrame() ?? "").toContain("required");
		stdin.write("5"); // typing clears the error and fills the field
		await wait(20);
		stdin.write("\r");
		await wait(20);
		expect(submitted).toEqual(["5"]);
	});

	it("applies initial overrides and submits defaults for untouched args", async () => {
		let submitted: string[] | null = null;
		const { lastFrame, stdin } = renderForm(
			[{ name: "source" }, { name: "target", default: "main" }],
			{ initial: { source: "feat-x" }, onSubmit: (v) => (submitted = v) },
		);
		await wait(20);
		expect(lastFrame() ?? "").toContain("feat-x");
		stdin.write("\r"); // source filled by initial, target by default
		await wait(20);
		expect(submitted).toEqual(["feat-x", "main"]);
	});

	it("ignores stray mouse reports (never lands in a field)", async () => {
		let submitted: string[] | null = null;
		const { lastFrame, stdin } = renderForm([{ name: "pr" }], {
			onSubmit: (v) => (submitted = v),
		});
		await wait(20);
		stdin.write("9");
		stdin.write("[<0;34;12M"); // SGR mouse report — must be dropped
		await wait(20);
		expect(lastFrame() ?? "").not.toContain("34");
		stdin.write("\r");
		await wait(20);
		expect(submitted).toEqual(["9"]);
	});

	it("esc cancels", async () => {
		let cancelled = false;
		const { stdin } = renderForm([{ name: "pr" }], {
			onCancel: () => {
				cancelled = true;
			},
		});
		await wait(20);
		stdin.write(ESC);
		await wait(20);
		expect(cancelled).toBe(true);
	});

	it("fixed args are read-only: focus skips them, typing edits the next row, value still submits", async () => {
		let submitted: string[] | null = null;
		const { lastFrame, stdin } = renderForm(
			[{ name: "source" }, { name: "target", default: "main" }],
			{ fixed: { source: "wt-a" }, onSubmit: (v) => (submitted = v) },
		);
		await wait(20);
		expect(lastFrame() ?? "").toContain("source> wt-a");
		// Focus starts on target (source is fixed); typing must land there.
		stdin.write("x");
		await wait(20);
		expect(lastFrame() ?? "").toContain("target> mainx");
		expect(lastFrame() ?? "").toContain("source> wt-a");
		// Tab wraps but skips the fixed row — still on target.
		stdin.write(TAB);
		stdin.write("y");
		await wait(20);
		expect(lastFrame() ?? "").toContain("target> mainxy");
		stdin.write("\r");
		await wait(20);
		expect(submitted).toEqual(["wt-a", "mainxy"]);
	});
});
