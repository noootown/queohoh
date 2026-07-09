import type { TaskDefinition } from "@queohoh/core";
import { Text } from "ink";
import { render } from "ink-testing-library";
import { describe, expect, it } from "vitest";
import type { DefinitionSummary } from "../actions.js";
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
			<QueuePane rows={rows} selectedIndex={0} focused={true} capacity={10} />,
		);
		expect(lastFrame()).toContain("QUEUE");
		expect(lastFrame()).toContain("▶");
		expect(lastFrame()).toContain("reply to review");
		expect(lastFrame()).toContain("#1 in lane");
	});

	it("renders the empty state", () => {
		const { lastFrame } = render(
			<QueuePane rows={[]} selectedIndex={0} focused={true} capacity={10} />,
		);
		expect(lastFrame()).toContain("queue empty — [f]/[m] on a worktree to add");
	});

	// Archived rows carry dimColor (stripped by the test harness), so the visible
	// intent we can pin is that both live and archived rows still render.
	it("renders both live and archived rows", () => {
		const live = render(
			<QueuePane
				rows={[queueRow("L", "live row text")]}
				selectedIndex={-1}
				focused={false}
				capacity={10}
			/>,
		).lastFrame();
		const archived = render(
			<QueuePane
				rows={[{ ...queueRow("A", "archived row text"), kind: "archived" }]}
				selectedIndex={-1}
				focused={false}
				capacity={10}
			/>,
		).lastFrame();
		expect(live).toContain("live row text");
		expect(archived).toContain("archived row text");
	});

	it("renders the chain marker for main-session rows", () => {
		const { lastFrame } = render(
			<QueuePane
				rows={[{ ...queueRow("M", "main row"), sessionMarker: "⛓ " }]}
				selectedIndex={-1}
				focused={false}
				capacity={10}
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
			<QueuePane rows={many} selectedIndex={4} focused={true} capacity={2} />,
		);
		expect(lastFrame()).toContain("row-4");
		expect(lastFrame()).not.toContain("row-0");
	});
});

describe("TasksPane", () => {
	const defs: DefinitionSummary[] = [
		{ repo: "platform", name: "review", args: [], hasDiscovery: true },
		{
			repo: "platform",
			name: "ticket",
			args: ["id", "flag"],
			hasDiscovery: false,
		},
	];

	it("renders name, args and discovery badge", () => {
		const { lastFrame } = render(
			<TasksPane defs={defs} selectedIndex={0} focused={true} capacity={10} />,
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
				selectedIndex={0}
				focused={true}
				capacity={10}
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
				selectedIndex={0}
				focused={false}
				capacity={10}
			/>,
		);
		const frame = lastFrame() ?? "";
		expect(frame).toContain("WORKTREES");
		const nameLines = frame.split("\n").filter((l) => l.includes("www"));
		expect(nameLines).toHaveLength(1);
	});
});

describe("list rows never wrap (title stays visible)", () => {
	it("QueuePane truncates long summaries to a single line", () => {
		const { lastFrame } = render(
			<QueuePane
				rows={[queueRow("L1", "s".repeat(300))]}
				selectedIndex={0}
				focused={false}
				capacity={10}
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
				args: [],
				hasDiscovery: false,
			},
		];
		const { lastFrame } = render(
			<TasksPane defs={defs} selectedIndex={0} focused={false} capacity={10} />,
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
				selectedIndex={-1}
				focused={false}
				capacity={10}
			/>,
		);
		expect(lastFrame()).toContain("◆");
	});
});

describe("Footer", () => {
	it("prefers the status line (red) over hints", () => {
		const { lastFrame } = render(
			<Footer focus="queue" prefixArmed={false} statusLine="boom" />,
		);
		expect(lastFrame()).toContain("boom");
		expect(lastFrame()).not.toContain("[a]dd");
	});

	it("shows the prefix indicator when armed", () => {
		const { lastFrame } = render(
			<Footer focus="queue" prefixArmed={true} statusLine={null} />,
		);
		expect(lastFrame()).toContain("PREFIX");
		expect(lastFrame()).toContain("n/p cycle");
	});

	it("shows queue hints without [a]dd", () => {
		const { lastFrame } = render(
			<Footer focus="queue" prefixArmed={false} statusLine={null} />,
		);
		expect(lastFrame()).not.toContain("[a]dd");
		expect(lastFrame()).toContain("[enter] detail");
		expect(lastFrame()).toContain("[q]uit");
	});

	it("shows tasks hints", () => {
		const { lastFrame } = render(
			<Footer focus="tasks" prefixArmed={false} statusLine={null} />,
		);
		expect(lastFrame()).toContain("[enter] run");
		expect(lastFrame()).not.toContain("[a]dd");
	});

	it("shows worktrees hints with fresh/main task keys", () => {
		const { lastFrame } = render(
			<Footer focus="worktrees" prefixArmed={false} statusLine={null} />,
		);
		expect(lastFrame()).toContain("[f]resh task");
		expect(lastFrame()).toContain("[m]ain task");
		expect(lastFrame()).toContain("[enter] run def");
	});

	it("shows detail hints", () => {
		const { lastFrame } = render(
			<Footer focus="detail" prefixArmed={false} statusLine={null} />,
		);
		expect(lastFrame()).toContain("sub-tab");
		expect(lastFrame()).toContain("[g/G] top/bottom");
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
		args: ["id", "flag"],
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
