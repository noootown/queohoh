import { laneKey, type SessionMode, type TaskDefinition } from "@queohoh/core";
import { Box, Text, useApp, useInput } from "ink";
import { useEffect, useMemo, useRef, useState } from "react";
import type { Actions, DefinitionSummary } from "./actions.js";
import { DetailPane } from "./components/DetailPane.js";
import { Footer } from "./components/Footer.js";
import {
	Modal,
	modalGeometry,
	modalInnerWidth,
	padLine,
} from "./components/Modal.js";
import { QueuePane } from "./components/QueuePanel.js";
import { TabBar } from "./components/TabBar.js";
import { TasksPane } from "./components/TasksPane.js";
import { TextInput } from "./components/TextInput.js";
import { WorktreesPane } from "./components/WorktreesPane.js";
import { anchorFor, clampSubTab, type DetailContext } from "./detail.js";
import type { KeyInput, KeymapAction, ListPaneId, PaneId } from "./keymap.js";
import { handleKey, moveFocus, parseMouseWheel } from "./keymap.js";
import { readRunFiles } from "./run-files.js";
import {
	buildProjectTabs,
	buildWorktreeRows,
	computePaneLayout,
	queueRowsForProject,
} from "./selectors.js";
import { useDaemon } from "./use-daemon.js";
import { useTerminalSize } from "./use-terminal-size.js";

type Mode =
	| { kind: "list" }
	| { kind: "add-task"; worktree: string; session: SessionMode }
	| { kind: "worktree-input"; taskId: string }
	| {
			kind: "def-pick";
			defs: DefinitionSummary[];
			index: number;
			worktree?: string;
	  }
	| { kind: "def-args"; def: DefinitionSummary; worktree?: string };

interface TabUiState {
	focus: PaneId;
	lastListPane: ListPaneId;
	selections: { queue: number; tasks: number; worktrees: number };
	subTab: Record<DetailContext["kind"], number>;
	scrollOffset: number;
}

const DEFAULT_UI: TabUiState = {
	focus: "queue",
	lastListPane: "queue",
	selections: { queue: 0, tasks: 0, worktrees: 0 },
	subTab: { run: 0, definition: 0, worktree: 0, empty: 0 },
	scrollOffset: 0,
};

function clampIdx(index: number, count: number): number {
	if (count <= 0) return 0;
	return Math.max(0, Math.min(index, count - 1));
}

export function App({
	sockPath,
	runsDir,
	actions,
	stdoutStream,
}: {
	sockPath: string;
	runsDir: string;
	actions: Actions;
	stdoutStream?: NodeJS.WriteStream;
}) {
	const { exit } = useApp();
	const { columns, rows } = useTerminalSize(stdoutStream);
	const { snapshot, connected } = useDaemon(sockPath);

	const [now, setNow] = useState(Date.now());
	const [activeTab, setActiveTab] = useState(0);
	const [uiByTab, setUiByTab] = useState<Record<string, TabUiState>>({});
	const [prefixArmed, setPrefixArmed] = useState(false);
	const prefixTimer = useRef<NodeJS.Timeout | null>(null);
	const [mode, setMode] = useState<Mode>({ kind: "list" });
	const [input, setInput] = useState("");
	const [statusLine, setStatusLine] = useState<string | null>(null);
	const [defsByProject, setDefsByProject] = useState<
		Record<string, DefinitionSummary[]>
	>({});
	const [fullDefs, setFullDefs] = useState<
		Record<string, TaskDefinition | null>
	>({});
	const [runFiles, setRunFiles] = useState<{
		report: string | null;
		transcriptTail: string[];
	} | null>(null);

	useEffect(() => {
		const timer = setInterval(() => setNow(Date.now()), 1000);
		return () => clearInterval(timer);
	}, []);

	// --- derived view model -------------------------------------------------
	const tabs = useMemo(
		() => (snapshot ? buildProjectTabs(snapshot) : []),
		[snapshot],
	);
	const activeIndex = Math.min(activeTab, Math.max(0, tabs.length - 1));
	const activeName = tabs[activeIndex]?.name ?? null;
	const ui = (activeName ? uiByTab[activeName] : undefined) ?? DEFAULT_UI;

	// Modals float absolutely over the body, so the body height is fixed and does
	// not reflow when a modal opens.
	const bodyHeight = Math.max(1, rows - 2);
	const { queueCap, listCap } = computePaneLayout(bodyHeight);
	const detailWidth = Math.max(20, Math.floor(columns * 0.62));
	const detailHeight = Math.max(1, bodyHeight - 4);
	// Modal width is independent of content height (see modalGeometry); the inner
	// width is what every child row pads to so the floating modal is opaque.
	const modalInner = modalInnerWidth(modalGeometry(columns, rows, 1).width);

	const queueRows = useMemo(
		() =>
			snapshot && activeName
				? queueRowsForProject(snapshot, activeName, now, 60)
				: [],
		[snapshot, activeName, now],
	);
	const defs = activeName ? (defsByProject[activeName] ?? []) : [];
	const wtRows = useMemo(
		() =>
			snapshot && activeName ? buildWorktreeRows(snapshot, activeName) : [],
		[snapshot, activeName],
	);

	const queueSel = clampIdx(ui.selections.queue, queueRows.length);
	const tasksSel = clampIdx(ui.selections.tasks, defs.length);
	const wtSel = clampIdx(ui.selections.worktrees, wtRows.length);

	const context: DetailContext = (() => {
		if (!snapshot || !activeName) return { kind: "empty" };
		if (ui.lastListPane === "queue") {
			const row = queueRows[queueSel];
			if (!row) return { kind: "empty" };
			const task = [...snapshot.tasks, ...snapshot.archivedRecent].find(
				(t) => t.id === row.id,
			);
			return task ? { kind: "run", task } : { kind: "empty" };
		}
		if (ui.lastListPane === "tasks") {
			const def = defs[tasksSel];
			return def
				? { kind: "definition", repo: def.repo, name: def.name }
				: { kind: "empty" };
		}
		const row = wtRows[wtSel];
		if (!row) return { kind: "empty" };
		const lane = `${activeName}:${row.name}`;
		const laneTasks = [...snapshot.tasks, ...snapshot.archivedRecent].filter(
			(t) => laneKey(t) === lane,
		);
		return { kind: "worktree", row, laneTasks };
	})();

	const subTab = clampSubTab(ui.subTab[context.kind], context.kind);
	const selDefRepo = context.kind === "definition" ? context.repo : null;
	const selDefName = context.kind === "definition" ? context.name : null;
	const selDefKey =
		selDefRepo && selDefName ? `${selDefRepo}/${selDefName}` : null;
	const detailDefinition = selDefKey ? (fullDefs[selDefKey] ?? null) : null;
	const runTaskId = context.kind === "run" ? context.task.id : null;

	// --- lazy fetches -------------------------------------------------------
	useEffect(() => {
		if (!activeName) return;
		if (defsByProject[activeName]) return;
		let cancelled = false;
		void actions.definitions().then((all) => {
			if (cancelled) return;
			setDefsByProject((prev) => ({
				...prev,
				[activeName]: all.filter((d) => d.repo === activeName),
			}));
		});
		return () => {
			cancelled = true;
		};
	}, [activeName, actions, defsByProject]);

	useEffect(() => {
		if (selDefRepo === null || selDefName === null) return;
		const key = `${selDefRepo}/${selDefName}`;
		if (key in fullDefs) return;
		let cancelled = false;
		void actions.definition(selDefRepo, selDefName).then((def) => {
			if (cancelled) return;
			setFullDefs((prev) => ({ ...prev, [key]: def }));
		});
		return () => {
			cancelled = true;
		};
	}, [selDefRepo, selDefName, actions, fullDefs]);

	useEffect(() => {
		if (runTaskId === null) {
			setRunFiles(null);
			return;
		}
		// Read more than the visible window so the detail pane has scrollback to
		// page through (offset-from-end into the tail buffer). Clamp >= 1 — a
		// tailLines of 0 hits a slice(-0) bug that returns the whole file.
		const tailLines = Math.max(1, detailHeight * 4);
		const read = () => {
			try {
				setRunFiles(readRunFiles(runsDir, runTaskId, { tailLines }));
			} catch {
				setRunFiles(null);
			}
		};
		read();
		const timer = setInterval(read, 1000);
		return () => clearInterval(timer);
	}, [runTaskId, runsDir, detailHeight]);

	// --- helpers ------------------------------------------------------------
	const patchTab = (fn: (state: TabUiState) => TabUiState) => {
		if (!activeName) return;
		setUiByTab((prev) => ({
			...prev,
			[activeName]: fn(prev[activeName] ?? DEFAULT_UI),
		}));
	};
	const act = (result: Promise<string | null>) => {
		void result.then((err) => setStatusLine(err));
	};
	const invalidateDefs = () => {
		if (!activeName) return;
		setDefsByProject((prev) => {
			const { [activeName]: _drop, ...rest } = prev;
			return rest;
		});
	};

	const dispatch = (action: KeymapAction) => {
		switch (action.type) {
			case "quit":
				exit();
				return;
			case "move-selection": {
				const pane = ui.focus as ListPaneId;
				const count =
					pane === "queue"
						? queueRows.length
						: pane === "tasks"
							? defs.length
							: wtRows.length;
				const cur =
					pane === "queue" ? queueSel : pane === "tasks" ? tasksSel : wtSel;
				const next = clampIdx(cur + action.delta, count);
				patchTab((s) => ({
					...s,
					selections: { ...s.selections, [pane]: next },
					scrollOffset: 0,
				}));
				return;
			}
			case "move-focus": {
				const next = moveFocus(ui.focus, action.dir, ui.lastListPane);
				patchTab((s) => ({
					...s,
					focus: next,
					lastListPane: next === "detail" ? s.lastListPane : next,
					scrollOffset: next === "detail" ? s.scrollOffset : 0,
				}));
				return;
			}
			case "focus": {
				const pane = action.pane;
				patchTab((s) => ({
					...s,
					focus: pane,
					lastListPane: pane === "detail" ? s.lastListPane : pane,
				}));
				return;
			}
			case "switch-tab": {
				if (action.index >= 0 && action.index < tabs.length) {
					setActiveTab(action.index);
				}
				return;
			}
			case "cycle-tab": {
				if (tabs.length === 0) return;
				setActiveTab((prev) => {
					const base = Math.min(prev, tabs.length - 1);
					return (base + action.delta + tabs.length) % tabs.length;
				});
				return;
			}
			case "switch-subtab": {
				const kind = context.kind;
				const idx = clampSubTab(action.index, kind);
				patchTab((s) => ({
					...s,
					subTab: { ...s.subTab, [kind]: idx },
					scrollOffset: 0,
				}));
				return;
			}
			case "worktree-add": {
				const row = wtRows[wtSel];
				if (row?.kind !== "worktree") return;
				setInput("");
				setMode({
					kind: "add-task",
					worktree: row.name,
					session: action.session,
				});
				return;
			}
			case "queue-retry": {
				const row = queueRows[queueSel];
				if (row && row.kind !== "archived") act(actions.retry(row.id));
				return;
			}
			case "queue-skip": {
				const row = queueRows[queueSel];
				if (row && row.kind !== "archived") act(actions.skip(row.id));
				return;
			}
			case "queue-worktree": {
				const row = queueRows[queueSel];
				if (row && row.kind !== "archived") {
					setInput("");
					setMode({ kind: "worktree-input", taskId: row.id });
				}
				return;
			}
			case "activate": {
				if (ui.focus === "tasks") {
					const def = defs[tasksSel];
					if (!def) return;
					if (def.args.length > 0) {
						setInput("");
						setMode({ kind: "def-args", def });
					} else {
						act(actions.runDefinition(def.repo, def.name, []));
						invalidateDefs();
					}
				} else if (ui.focus === "worktrees") {
					const row = wtRows[wtSel];
					if (row?.kind !== "worktree") return;
					if (defs.length === 0) {
						setStatusLine("no task definitions found");
						return;
					}
					setMode({
						kind: "def-pick",
						defs,
						index: 0,
						worktree: row.name,
					});
				}
				return;
			}
			case "scroll": {
				// Bottom-anchored views (the run transcript tail) invert scroll so
				// ↑/k moves into history (older) and ↓/j returns toward the live tail;
				// top-anchored views keep the natural ↓ = down mapping.
				const bottomAnchored = anchorFor(context.kind, subTab) === "bottom";
				const step = bottomAnchored ? -action.delta : action.delta;
				patchTab((s) => ({
					...s,
					scrollOffset: Math.max(0, s.scrollOffset + step),
				}));
				return;
			}
			case "scroll-edge": {
				// g (edge "top") jumps to head/oldest, G (edge "bottom") to tail/end.
				// On a bottom-anchored view the head is the far scrollback (large
				// offset) and the tail is offset 0; top-anchored views are the reverse.
				// windowLines clamps the large sentinel to the real max offset.
				const bottomAnchored = anchorFor(context.kind, subTab) === "bottom";
				const toHead = action.edge === "top";
				const offset = bottomAnchored
					? toHead
						? 1_000_000
						: 0
					: toHead
						? 0
						: 1_000_000;
				patchTab((s) => ({ ...s, scrollOffset: offset }));
				return;
			}
		}
	};

	useInput((char, key) => {
		if (mode.kind === "def-pick") {
			if (key.escape || char === "q") {
				setMode({ kind: "list" });
			} else if (key.upArrow || char === "k") {
				setMode({ ...mode, index: Math.max(0, mode.index - 1) });
			} else if (key.downArrow || char === "j") {
				setMode({
					...mode,
					index: Math.min(mode.defs.length - 1, mode.index + 1),
				});
			} else if (key.return) {
				const def = mode.defs[mode.index];
				if (!def) return;
				if (def.args.length > 0) {
					setInput("");
					setMode({ kind: "def-args", def, worktree: mode.worktree });
				} else {
					act(actions.runDefinition(def.repo, def.name, [], mode.worktree));
					invalidateDefs();
					setMode({ kind: "list" });
				}
			}
			return;
		}
		if (mode.kind !== "list") return; // text inputs handled by TextInput

		// Mouse wheel scrolls the focused pane: detail scrolls its content, the
		// list panes move their selection (which scrolls the row window) — the
		// same mapping as ↑/↓ / j/k.
		const wheel = parseMouseWheel(char);
		if (wheel) {
			const delta = wheel === "down" ? 1 : -1;
			dispatch(
				ui.focus === "detail"
					? { type: "scroll", delta }
					: { type: "move-selection", delta },
			);
			return;
		}

		setStatusLine(null);
		if (prefixTimer.current) {
			clearTimeout(prefixTimer.current);
			prefixTimer.current = null;
		}
		const keyInput: KeyInput = {
			input: char,
			ctrl: key.ctrl,
			upArrow: key.upArrow,
			downArrow: key.downArrow,
			leftArrow: key.leftArrow,
			rightArrow: key.rightArrow,
			return: key.return,
		};
		const result = handleKey(prefixArmed, ui.focus, keyInput);
		setPrefixArmed(result.prefixArmed);
		if (result.prefixArmed) {
			prefixTimer.current = setTimeout(() => setPrefixArmed(false), 2000);
		}
		if (result.action) dispatch(result.action);
	});

	if (columns < 60 || rows < 15) {
		return (
			<Box width={columns} height={rows}>
				<Text>terminal too small (60x15 minimum)</Text>
			</Box>
		);
	}

	return (
		<Box
			width={columns}
			height={rows}
			flexDirection="column"
			position="relative"
		>
			<TabBar
				tabs={tabs}
				activeIndex={activeIndex}
				connected={connected}
				runningCount={snapshot?.running.length ?? 0}
				maxConcurrent={snapshot?.maxConcurrent ?? null}
			/>
			<Box flexGrow={1}>
				<Box width="34%" flexShrink={0} flexDirection="column">
					<QueuePane
						rows={queueRows}
						selectedIndex={queueSel}
						focused={ui.focus === "queue"}
						capacity={queueCap}
					/>
					<TasksPane
						defs={defs}
						selectedIndex={tasksSel}
						focused={ui.focus === "tasks"}
						capacity={listCap}
					/>
					<WorktreesPane
						rows={wtRows}
						selectedIndex={wtSel}
						focused={ui.focus === "worktrees"}
						capacity={listCap}
					/>
				</Box>
				<DetailPane
					context={context}
					subTab={subTab}
					focused={ui.focus === "detail"}
					width={detailWidth}
					height={detailHeight}
					scrollOffset={ui.scrollOffset}
					runFiles={runFiles}
					definition={detailDefinition}
				/>
			</Box>
			<Footer
				focus={ui.focus}
				prefixArmed={prefixArmed}
				statusLine={statusLine}
			/>
			{mode.kind === "add-task" ? (
				<Modal
					title={`New task — ${mode.session} session — ${activeName}:${mode.worktree}`}
					columns={columns}
					rows={rows}
					hint="enter submit · esc cancel"
				>
					<TextInput
						label="prompt"
						value={input}
						width={modalInner}
						onChange={setInput}
						onSubmit={(v) => {
							if (activeName)
								act(
									actions.enqueue(v, activeName, {
										worktree: mode.worktree,
										session: mode.session,
									}),
								);
							setInput("");
							setMode({ kind: "list" });
						}}
						onCancel={() => setMode({ kind: "list" })}
					/>
				</Modal>
			) : null}
			{mode.kind === "worktree-input" ? (
				<Modal
					title={`Assign worktree — task ${mode.taskId.slice(-6)}`}
					columns={columns}
					rows={rows}
					hint="enter submit · esc cancel"
				>
					<TextInput
						label="worktree"
						value={input}
						width={modalInner}
						onChange={setInput}
						onSubmit={(v) => {
							act(actions.setWorktree(mode.taskId, v));
							setInput("");
							setMode({ kind: "list" });
						}}
						onCancel={() => setMode({ kind: "list" })}
					/>
				</Modal>
			) : null}
			{mode.kind === "def-args" ? (
				<Modal
					title={`${mode.def.name} args (${mode.def.args.join(" ")})`}
					columns={columns}
					rows={rows}
					hint="enter run · esc cancel"
				>
					<TextInput
						label="args"
						value={input}
						width={modalInner}
						onChange={setInput}
						onSubmit={(v) => {
							act(
								actions.runDefinition(
									mode.def.repo,
									mode.def.name,
									v.trim().length > 0 ? v.trim().split(/\s+/) : [],
									mode.worktree,
								),
							);
							invalidateDefs();
							setInput("");
							setMode({ kind: "list" });
						}}
						onCancel={() => setMode({ kind: "list" })}
					/>
				</Modal>
			) : null}
			{mode.kind === "def-pick" ? (
				<Modal
					title={`Run task definition — ${
						mode.worktree ? `${activeName}:${mode.worktree}` : activeName
					}`}
					columns={columns}
					rows={rows}
					hint="↑/↓ move · enter run · q/esc close"
				>
					{mode.defs.map((def, i) => (
						<Text key={`${def.repo}/${def.name}`} inverse={i === mode.index}>
							{padLine(
								` ${def.name}${
									def.args.length > 0 ? ` (${def.args.join(", ")})` : ""
								}${def.hasDiscovery ? " ⏰" : ""}`,
								modalInner,
							)}
						</Text>
					))}
				</Modal>
			) : null}
		</Box>
	);
}
