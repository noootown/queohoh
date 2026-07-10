import {
	contextArgValues,
	laneKey,
	type SessionMode,
	type TaskDefinition,
} from "@queohoh/core";
import { currentBuildId } from "@queohoh/daemon";
import { Box, Text, useApp, useInput } from "ink";
import { useEffect, useMemo, useRef, useState } from "react";
import {
	type ActionId,
	type ActionItem,
	buildActions,
	buildBulkActions,
} from "./action-menu.js";
import { type Actions, argSummary, type DefinitionSummary } from "./actions.js";
import { validateBranchName } from "./branch.js";
import { ArgsForm } from "./components/ArgsForm.js";
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
import { stripRepoPrefix } from "./format.js";
import { decideHeal, isStale, performHeal } from "./heal.js";
import type { KeyInput, KeymapAction, ListPaneId, PaneId } from "./keymap.js";
import {
	handleKey,
	isMouseEvent,
	moveFocus,
	parseMouseWheel,
} from "./keymap.js";
import { readRunFiles } from "./run-files.js";
import {
	ambientRunArgs,
	buildProjectTabs,
	buildWorktreeRows,
	clampSelection,
	computePaneLayout,
	matchesFilter,
	type PaneSelection,
	queueRowsForProject,
	selectionRange,
} from "./selectors.js";
import { insideTmux, openTmuxWindow } from "./tmux.js";
import { useDaemon } from "./use-daemon.js";
import { useTerminalSize } from "./use-terminal-size.js";

/** Files backing a single run, tagged with the task they belong to so a stale
 * read for a just-abandoned selection can be told apart from the live one. */
type RunFiles = {
	taskId: string;
	report: string | null;
	transcriptTail: string[];
};

/**
 * Cheap content-equality for run files: same task, same report text, same
 * transcript tail. Lets the debounced read and the 1s poll skip `setRunFiles`
 * (and the render it triggers) when the files on disk have not changed since the
 * last read — a slowly-streaming task re-reads identical bytes every second.
 */
function sameRunFiles(a: RunFiles | null, b: RunFiles | null): boolean {
	if (a === b) return true;
	if (a === null || b === null) return false;
	return (
		a.taskId === b.taskId &&
		a.report === b.report &&
		a.transcriptTail.length === b.transcriptTail.length &&
		a.transcriptTail.join("\n") === b.transcriptTail.join("\n")
	);
}

type MenuTarget =
	| { kind: "queue"; taskId: string }
	| { kind: "task"; def: DefinitionSummary }
	| {
			kind: "worktree";
			/** stripped display name (modal titles) */
			name: string;
			/** raw `<repo>.<branch>` identifier for daemon dispatch (refs, removal) */
			rawName: string;
			path: string;
			branch: string | null;
	  }
	| { kind: "session"; path: string }
	| { kind: "bulk-queue"; rerunIds: string[]; skipIds: string[] }
	| { kind: "bulk-tasks"; defs: DefinitionSummary[] }
	| { kind: "bulk-worktrees"; names: string[] };

type Mode =
	| { kind: "list" }
	| { kind: "add-task"; worktree: string; session: SessionMode }
	| { kind: "worktree-input"; taskId: string }
	| {
			kind: "def-pick";
			defs: DefinitionSummary[];
			index: number;
			worktree?: string;
			/** branch of the targeted worktree, carried through so the chosen def's
			 * args form can auto-fill `source`/`branch`/`ticket` from it. */
			branch?: string | null;
	  }
	| {
			kind: "def-args";
			def: DefinitionSummary;
			worktree?: string;
			initial?: Record<string, string>;
			fixed?: Record<string, string>;
	  }
	| {
			kind: "action-menu";
			items: ActionItem[];
			index: number;
			target: MenuTarget;
			title: string;
	  }
	| { kind: "confirm-remove"; worktree: string; branch: string | null }
	| { kind: "create-worktree"; error?: string }
	| { kind: "confirm-bulk-remove"; names: string[] }
	| { kind: "search"; pane: ListPaneId };

interface TabUiState {
	focus: PaneId;
	lastListPane: ListPaneId;
	selections: Record<ListPaneId, PaneSelection>;
	search: Record<ListPaneId, string>;
	subTab: Record<DetailContext["kind"], number>;
	scrollOffset: number;
}

const DEFAULT_UI: TabUiState = {
	focus: "queue",
	lastListPane: "queue",
	selections: {
		queue: { cursor: 0, anchor: null },
		tasks: { cursor: 0, anchor: null },
		worktrees: { cursor: 0, anchor: null },
	},
	search: { queue: "", tasks: "", worktrees: "" },
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
	const [runFiles, setRunFiles] = useState<RunFiles | null>(null);
	// Mirror of the committed `runFiles`, kept fresh below. The 1s poll reads it to
	// decide equality SYNCHRONOUSLY and skip `setRunFiles` outright when the files
	// are unchanged — a same-value setState would still re-run App once.
	const runFilesRef = useRef<RunFiles | null>(runFiles);
	runFilesRef.current = runFiles;

	// `now` feeds ONLY the elapsed-time detail on RUNNING queue rows of the active
	// project (queueRowsForProject → format.elapsed). This ref, refreshed each
	// render below, lets the stable 1s interval bail out of updating `now` when
	// nothing is running — without re-subscribing the interval on every snapshot.
	const activeHasRunningRef = useRef(false);
	useEffect(() => {
		const timer = setInterval(() => {
			// Nothing running → every elapsed label is static, so don't touch state.
			// Reading the flag here (not inside setNow) means an idle tick does no
			// setState at all: 0 renders/sec at true idle. A same-value setNow would
			// still re-run App once (React re-renders a state owner before bailing).
			if (!activeHasRunningRef.current) return;
			// Running rows show per-second elapsed, so bucket to whole seconds; at a
			// 1s cadence this always advances, but it is the honest display grain.
			setNow((prev) =>
				Math.floor(prev / 1000) === Math.floor(Date.now() / 1000)
					? prev
					: Date.now(),
			);
		}, 1000);
		return () => clearInterval(timer);
	}, []);

	// --- daemon self-heal ---------------------------------------------------
	// The daemon runs detached; after a rebuild the old process keeps serving
	// stale code (→ "unknown method" on new RPCs). Every snapshot carries the
	// daemon's buildId; compare it to what's on disk and, when idle, restart the
	// daemon so it ends up on the latest build without manual intervention.
	const lastHealedBuildId = useRef<string | null>(null);
	const healing = useRef(false);
	// True while the current status line was written by this effect — the
	// healthy branch may only clear its own messages, never unrelated ones.
	const healStatusShown = useRef(false);
	useEffect(() => {
		if (!connected || !snapshot) return;
		const setHealStatus = (line: string) => {
			healStatusShown.current = true;
			setStatusLine(line);
		};
		const disk = currentBuildId();
		const action = decideHeal({
			snapshotBuildId: snapshot.buildId,
			diskBuildId: disk,
			runningCount: snapshot.running.length,
			lastHealedBuildId: lastHealedBuildId.current,
		});
		if (action === "none") {
			if (isStale(snapshot.buildId, disk)) {
				// Stale but decideHeal declined: we already tried this build and it
				// didn't take — stop retrying and say so. Suppressed while a restart
				// is mid-flight (a lingering old-daemon push must not raise a false
				// alarm before the fresh daemon connects).
				if (!healing.current) {
					setHealStatus("daemon still outdated — restart it manually");
				}
			} else {
				// Healthy: reset the guard so a future rebuild heals again, and
				// clear our own status (e.g. "restarting…" after a successful heal).
				lastHealedBuildId.current = null;
				if (healStatusShown.current) {
					healStatusShown.current = false;
					setStatusLine(null);
				}
			}
			return;
		}
		if (action === "defer") {
			setHealStatus("daemon outdated — will restart when idle");
			return;
		}
		// restart-now: record the attempt (loop guard) before firing.
		lastHealedBuildId.current = disk;
		healing.current = true;
		setHealStatus("daemon outdated — restarting…");
		void performHeal({ sockPath }).then((ok) => {
			healing.current = false;
			if (!ok) setHealStatus("daemon busy — restart deferred");
			// On success the reconnect loop picks up the fresh daemon; its healthy
			// snapshot clears this status via the branch above.
		});
	}, [snapshot, connected, sockPath]);

	// --- derived view model -------------------------------------------------
	const tabs = useMemo(
		() => (snapshot ? buildProjectTabs(snapshot) : []),
		[snapshot],
	);
	const activeIndex = Math.min(activeTab, Math.max(0, tabs.length - 1));
	const activeName = tabs[activeIndex]?.name ?? null;
	const ui = (activeName ? uiByTab[activeName] : undefined) ?? DEFAULT_UI;

	// Refresh the flag the now-tick reads (see the interval above). Cheap `.some`
	// over the task list; only the active project's running rows animate elapsed.
	activeHasRunningRef.current =
		!!snapshot &&
		activeName !== null &&
		snapshot.tasks.some(
			(t) => t.target.repo === activeName && t.status === "running",
		);

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

	const visibleQueueRows = useMemo(
		() => queueRows.filter((r) => matchesFilter(r.summary, ui.search.queue)),
		[queueRows, ui.search.queue],
	);
	const visibleDefs = useMemo(
		() => defs.filter((d) => matchesFilter(d.name, ui.search.tasks)),
		[defs, ui.search.tasks],
	);
	const visibleWtRows = useMemo(
		() => wtRows.filter((r) => matchesFilter(r.name, ui.search.worktrees)),
		[wtRows, ui.search.worktrees],
	);

	const queueSel = clampSelection(ui.selections.queue, visibleQueueRows.length);
	const tasksSel = clampSelection(ui.selections.tasks, visibleDefs.length);
	const wtSel = clampSelection(ui.selections.worktrees, visibleWtRows.length);

	const visibleCount = (pane: ListPaneId): number =>
		pane === "queue"
			? visibleQueueRows.length
			: pane === "tasks"
				? visibleDefs.length
				: visibleWtRows.length;
	const paneSel = (pane: ListPaneId): PaneSelection =>
		pane === "queue" ? queueSel : pane === "tasks" ? tasksSel : wtSel;

	// Memoized so its stable reference lets React.memo(DetailPane) skip renders
	// where the selection is unchanged (e.g. the now-tick, or an unrelated modal
	// toggle). A fresh object every render would defeat that memo.
	const context: DetailContext = useMemo(() => {
		if (!snapshot || !activeName) return { kind: "empty" };
		if (ui.lastListPane === "queue") {
			const row = visibleQueueRows[queueSel.cursor];
			if (!row) return { kind: "empty" };
			const task = [...snapshot.tasks, ...snapshot.archivedRecent].find(
				(t) => t.id === row.id,
			);
			return task ? { kind: "run", task } : { kind: "empty" };
		}
		if (ui.lastListPane === "tasks") {
			const def = visibleDefs[tasksSel.cursor];
			return def
				? { kind: "definition", repo: def.repo, name: def.name }
				: { kind: "empty" };
		}
		const row = visibleWtRows[wtSel.cursor];
		if (!row) return { kind: "empty" };
		// `laneKey` keys on the raw `target.worktree` (the `<repo>.<branch>` name),
		// so the lane must be built from `rawName`, not the stripped display name.
		const lane = `${activeName}:${row.rawName}`;
		const laneTasks = [...snapshot.tasks, ...snapshot.archivedRecent].filter(
			(t) => laneKey(t) === lane,
		);
		return { kind: "worktree", row, laneTasks };
	}, [
		snapshot,
		activeName,
		ui.lastListPane,
		visibleQueueRows,
		queueSel,
		visibleDefs,
		tasksSel,
		visibleWtRows,
		wtSel,
	]);

	const subTab = clampSubTab(ui.subTab[context.kind], context.kind);
	const selDefRepo = context.kind === "definition" ? context.repo : null;
	const selDefName = context.kind === "definition" ? context.name : null;
	const selDefKey =
		selDefRepo && selDefName ? `${selDefRepo}/${selDefName}` : null;
	const detailDefinition = selDefKey ? (fullDefs[selDefKey] ?? null) : null;
	const runTaskId = context.kind === "run" ? context.task.id : null;

	// Only surface files that belong to the *current* selection. While a new
	// selection's debounced read is still in flight, `runFiles` may hold the
	// previous task's files — gate on taskId so the detail pane shows its loading
	// placeholder rather than another task's stale transcript for a beat.
	const currentRunFiles =
		runFiles && runFiles.taskId === runTaskId ? runFiles : null;

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
			// Guard the setState so leaving a run for a non-run selection doesn't add
			// a redundant App re-run (same-value setState still re-renders the owner).
			if (runFilesRef.current !== null) {
				runFilesRef.current = null;
				setRunFiles(null);
			}
			return;
		}
		// Read more than the visible window so the detail pane has scrollback to
		// page through (offset-from-end into the tail buffer). Clamp >= 1 — a
		// tailLines of 0 hits a slice(-0) bug that returns the whole file.
		const tailLines = Math.max(1, detailHeight * 4);
		const taskId = runTaskId;
		const readOnce = () => {
			let next: RunFiles;
			try {
				next = { taskId, ...readRunFiles(runsDir, taskId, { tailLines }) };
			} catch {
				next = { taskId, report: null, transcriptTail: [] };
			}
			// Content-identical read → do nothing (the 1s poll re-reads the same
			// bytes every second for a slowly-streaming task). Skipping the setState
			// entirely — not just returning prev from an updater — keeps a quiet poll
			// at 0 renders.
			if (sameRunFiles(runFilesRef.current, next)) return;
			runFilesRef.current = next;
			setRunFiles(next);
		};
		// Debounce the initial read: holding an arrow key through N tasks re-runs
		// this effect per keypress, and each run clears the still-pending timer, so
		// no file is read until the selection settles (~120ms of quiet). The 1s
		// poll starts only after that settle read, then keeps the tail fresh.
		let poll: NodeJS.Timeout | null = null;
		const debounce = setTimeout(() => {
			readOnce();
			poll = setInterval(readOnce, 1000);
		}, 120);
		return () => {
			clearTimeout(debounce);
			if (poll) clearInterval(poll);
		};
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

	// Sequential on purpose: one daemon socket, deterministic order, and each
	// item's error is independently reported into the summary line.
	const runBulk = async <T,>(
		items: T[],
		verb: string,
		fn: (item: T) => Promise<string | null>,
	): Promise<void> => {
		let failed = 0;
		let firstError: string | null = null;
		for (const item of items) {
			const err = await fn(item);
			if (err !== null) {
				failed += 1;
				firstError = firstError ?? err;
			}
		}
		const ok = items.length - failed;
		setStatusLine(
			failed === 0
				? `${verb} ${ok}`
				: `${verb} ${ok}, ${failed} failed: ${firstError}`,
		);
	};

	const clearRange = (pane: ListPaneId) =>
		patchTab((s) => ({
			...s,
			selections: {
				...s.selections,
				[pane]: { cursor: s.selections[pane].cursor, anchor: null },
			},
		}));

	// Bulk targets are frozen at menu-open time: the id/name lists captured here
	// are what executes, so daemon pushes that reshuffle rows mid-menu cannot
	// retarget the batch.
	const openBulkMenu = (
		pane: ListPaneId,
		start: number,
		end: number,
	): Mode | null => {
		const total = end - start + 1;
		if (pane === "queue") {
			const rows = visibleQueueRows.slice(start, end + 1);
			const statusById = new Map(
				(snapshot?.tasks ?? []).map((t) => [t.id, t.status]),
			);
			const live = rows.filter((r) => r.kind === "live");
			const rerunIds = live
				.filter((r) => {
					const s = statusById.get(r.id);
					return s === "failed" || s === "needs-input";
				})
				.map((r) => r.id);
			const skipIds = live
				.filter((r) => {
					const s = statusById.get(r.id);
					return s === "failed" || s === "needs-input" || s === "done";
				})
				.map((r) => r.id);
			return {
				kind: "action-menu",
				items: buildBulkActions({
					kind: "bulk-queue",
					rerun: rerunIds.length,
					skip: skipIds.length,
					total,
				}),
				index: 0,
				target: { kind: "bulk-queue", rerunIds, skipIds },
				title: `${total} selected`,
			};
		}
		if (pane === "tasks") {
			const rows = visibleDefs.slice(start, end + 1);
			const runnable = rows.filter((d) => d.args.length === 0);
			return {
				kind: "action-menu",
				items: buildBulkActions({
					kind: "bulk-tasks",
					run: runnable.length,
					total,
				}),
				index: 0,
				target: { kind: "bulk-tasks", defs: runnable },
				title: `${total} selected`,
			};
		}
		const rows = visibleWtRows.slice(start, end + 1);
		const removable = rows.filter(
			(r) => r.kind === "worktree" && r.state !== "busy",
		);
		return {
			kind: "action-menu",
			items: buildBulkActions({
				kind: "bulk-worktrees",
				remove: removable.length,
				total,
			}),
			index: 0,
			target: { kind: "bulk-worktrees", names: removable.map((r) => r.name) },
			title: `${total} selected`,
		};
	};

	// The action menu targets the same item the detail pane shows: the last
	// focused list pane's selection (context is already derived exactly that way).
	const openActionMenu = (): Mode | null => {
		const pane = ui.lastListPane;
		const { start, end } = selectionRange(paneSel(pane));
		if (end > start) return openBulkMenu(pane, start, end);
		if (context.kind === "run") {
			const row = visibleQueueRows[queueSel.cursor];
			if (!row) return null;
			return {
				kind: "action-menu",
				items: buildActions({
					kind: "queue",
					status: context.task.status,
					archived: row.kind === "archived",
				}),
				index: 0,
				target: { kind: "queue", taskId: context.task.id },
				title: row.summary,
			};
		}
		if (context.kind === "definition") {
			const def = visibleDefs.find(
				(d) => d.repo === context.repo && d.name === context.name,
			);
			if (!def) return null;
			return {
				kind: "action-menu",
				items: buildActions({ kind: "task" }),
				index: 0,
				target: { kind: "task", def },
				title: def.name,
			};
		}
		if (context.kind === "worktree") {
			const row = context.row;
			if (row.kind === "session") {
				return {
					kind: "action-menu",
					items: buildActions({ kind: "session", insideTmux: insideTmux() }),
					index: 0,
					target: { kind: "session", path: row.path },
					title: row.name,
				};
			}
			return {
				kind: "action-menu",
				items: buildActions({
					kind: "worktree",
					busy: row.state === "busy",
					insideTmux: insideTmux(),
					hasBranch: row.branch !== null,
				}),
				index: 0,
				target: {
					kind: "worktree",
					name: row.name,
					rawName: row.rawName,
					path: row.path,
					branch: row.branch,
				},
				title: row.name,
			};
		}
		return null;
	};

	const runMenuAction = (id: ActionId, target: MenuTarget) => {
		setMode({ kind: "list" });
		const pane = ui.lastListPane;
		switch (id) {
			case "rerun":
				if (target.kind === "queue") act(actions.retry(target.taskId));
				if (target.kind === "bulk-queue") {
					clearRange(pane);
					void runBulk(target.rerunIds, "reran", (taskId) =>
						actions.retry(taskId),
					);
				}
				return;
			case "skip":
				if (target.kind === "queue") act(actions.skip(target.taskId));
				if (target.kind === "bulk-queue") {
					clearRange(pane);
					void runBulk(target.skipIds, "skipped", (taskId) =>
						actions.skip(taskId),
					);
				}
				return;
			case "assign-worktree":
				if (target.kind === "queue") {
					setInput("");
					setMode({ kind: "worktree-input", taskId: target.taskId });
				}
				return;
			case "run": {
				if (target.kind === "bulk-tasks") {
					clearRange(pane);
					void runBulk(target.defs, "started", (d) =>
						actions.runDefinition(d.repo, d.name, []),
					).then(() => invalidateDefs());
					return;
				}
				if (target.kind !== "task") return;
				if (target.def.args.length > 0) {
					setInput("");
					// Ambient run: the run wasn't targeted at a worktree, so offer the
					// repo's branches as a `source` dropdown and borrow the worktrees-pane
					// selection as an *editable* prefill (explicit targeting would be
					// `fixed`). No worktree override — the def's own `worktree:` field
					// governs dispatch. The injected `options` are TUI-side only; the def
					// declares none, so the daemon never validates them and submission
					// stays positional.
					const { args, initial } = ambientRunArgs(
						target.def.args,
						visibleWtRows,
						visibleWtRows[wtSel.cursor],
					);
					setMode({
						kind: "def-args",
						// Shallow-copy the def with overlaid args so def-args' existing
						// render/submit path is unchanged; repo/name identity is kept.
						def: { ...target.def, args },
						...(Object.keys(initial).length > 0 ? { initial } : {}),
					});
				} else {
					act(actions.runDefinition(target.def.repo, target.def.name, []));
					invalidateDefs();
				}
				return;
			}
			case "task-fresh":
			case "task-main":
				if (target.kind === "worktree") {
					setInput("");
					setMode({
						kind: "add-task",
						// Raw identifier: enqueue builds ref `worktree:<name>` in the
						// daemon, which matches against the real `<repo>.<branch>` name.
						worktree: target.rawName,
						session: id === "task-fresh" ? "fresh" : "main",
					});
				}
				return;
			case "run-def":
				if (target.kind !== "worktree") return;
				if (defs.length === 0) {
					setStatusLine("no task definitions found");
					return;
				}
				// Carry the worktree's branch so the picked def's args form auto-fills
				// `source`/`branch`/`ticket` from it (fixed — this worktree is the
				// explicit target).
				setMode({
					kind: "def-pick",
					defs,
					index: 0,
					// Raw identifier: this flows to runDefinition's worktree override
					// (ref `worktree:<name>`), which resolves against the real name.
					worktree: target.rawName,
					branch: target.branch,
				});
				return;
			case "tmux-open":
				if (target.kind === "worktree" || target.kind === "session") {
					act(openTmuxWindow(target.path));
				}
				return;
			case "squash-merge": {
				if (target.kind !== "worktree" || !target.branch || !activeName) return;
				const repo = activeName;
				const branch = target.branch;
				// Fetch fresh so a workspace that just gained the global def is picked
				// up; the global squash-merge is keyed on the active project's repo.
				void actions.definitions().then((all) => {
					const def = all.find(
						(d) => d.repo === repo && d.name === "squash-merge",
					);
					if (!def) {
						setStatusLine(
							"squash-merge definition not found — copy library/tasks/squash-merge to <workspace>/global/tasks/",
						);
						return;
					}
					setInput("");
					// No worktree override: the def's `worktree: repo` governs, so the
					// task runs in the primary checkout, not the selected worktree.
					// `source` is decided by the selected worktree — fixed, not asked
					// (same convention every worktree-targeted run uses).
					setMode({ kind: "def-args", def, fixed: contextArgValues(branch) });
				});
				return;
			}
			case "remove-worktree":
				if (target.kind === "worktree") {
					setMode({
						kind: "confirm-remove",
						// Raw identifier for removeWorktree; the engine tolerates the
						// stripped form here too, but pass raw for one consistent contract.
						worktree: target.rawName,
						branch: target.branch,
					});
				}
				if (target.kind === "bulk-worktrees") {
					setMode({ kind: "confirm-bulk-remove", names: target.names });
				}
				return;
			case "create-worktree":
				setInput("");
				setMode({ kind: "create-worktree" });
				return;
			default: {
				const _exhaustive: never = id;
				return _exhaustive;
			}
		}
	};

	const dispatch = (action: KeymapAction) => {
		switch (action.type) {
			case "quit":
				exit();
				return;
			case "open-action-menu": {
				const opened = openActionMenu();
				if (opened === null) setStatusLine("nothing selected");
				else setMode(opened);
				return;
			}
			case "create": {
				setInput("");
				if (ui.lastListPane === "worktrees") {
					setMode({ kind: "create-worktree" });
				} else if (ui.lastListPane === "queue") {
					setMode({ kind: "add-task", worktree: "", session: "fresh" });
				}
				return;
			}
			case "open-search": {
				if (ui.focus === "detail") return;
				setMode({ kind: "search", pane: ui.focus });
				return;
			}
			case "clear-search": {
				if (ui.focus === "detail") return;
				const pane = ui.focus;
				if (paneSel(pane).anchor !== null) {
					// first Esc clears the range; a second Esc clears the filter
					const cursor = paneSel(pane).cursor;
					patchTab((s) => ({
						...s,
						selections: { ...s.selections, [pane]: { cursor, anchor: null } },
					}));
					return;
				}
				patchTab((s) => ({
					...s,
					search: { ...s.search, [pane]: "" },
					selections: {
						...s.selections,
						[pane]: { cursor: 0, anchor: null },
					},
				}));
				return;
			}
			case "move-selection": {
				const pane = ui.focus as ListPaneId;
				const next = clampIdx(
					paneSel(pane).cursor + action.delta,
					visibleCount(pane),
				);
				patchTab((s) => ({
					...s,
					selections: {
						...s.selections,
						[pane]: { cursor: next, anchor: null },
					},
					scrollOffset: 0,
				}));
				return;
			}
			case "extend-selection": {
				if (ui.focus === "detail") return;
				const pane = ui.focus as ListPaneId;
				const cur = paneSel(pane);
				const next = clampIdx(cur.cursor + action.delta, visibleCount(pane));
				// Collapse the anchor when the range shrinks back onto a single row,
				// so a subsequent Esc falls through to the filter-clear branch
				// instead of being silently consumed clearing an invisible range.
				const base = cur.anchor ?? cur.cursor;
				const anchor = next === base ? null : base;
				patchTab((s) => ({
					...s,
					selections: {
						...s.selections,
						[pane]: { cursor: next, anchor },
					},
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
					// This worktree is the explicit target, so its branch drives
					// `source`/`branch`/`ticket` as fixed rows (ArgsForm ignores keys the
					// def doesn't declare). Omit `fixed` when the branch implies nothing.
					const fixed = contextArgValues(mode.branch);
					setMode({
						kind: "def-args",
						def,
						worktree: mode.worktree,
						...(Object.keys(fixed).length > 0 ? { fixed } : {}),
					});
				} else {
					act(actions.runDefinition(def.repo, def.name, [], mode.worktree));
					invalidateDefs();
					setMode({ kind: "list" });
				}
			}
			return;
		}
		if (mode.kind === "action-menu") {
			if (key.escape || char === "q") {
				setMode({ kind: "list" });
			} else if (key.upArrow || char === "k") {
				setMode({ ...mode, index: Math.max(0, mode.index - 1) });
			} else if (key.downArrow || char === "j") {
				setMode({
					...mode,
					index: Math.min(mode.items.length - 1, mode.index + 1),
				});
			} else if (key.return) {
				const item = mode.items[mode.index];
				// disabled rows are selectable but inert — the menu shape stays stable
				if (item && item.disabled === undefined) {
					runMenuAction(item.id, mode.target);
				}
			}
			return;
		}
		if (mode.kind === "confirm-remove") {
			if (char === "y") {
				if (activeName) act(actions.removeWorktree(activeName, mode.worktree));
				setMode({ kind: "list" });
			} else if (char === "n" || char === "q" || key.escape) {
				setMode({ kind: "list" });
			}
			return;
		}
		if (mode.kind === "confirm-bulk-remove") {
			if (char === "y") {
				const repo = activeName;
				const names = mode.names;
				if (repo !== null) {
					clearRange("worktrees");
					void runBulk(names, "removed", (name) =>
						actions.removeWorktree(repo, name),
					);
				}
				setMode({ kind: "list" });
			} else if (char === "n" || char === "q" || key.escape) {
				setMode({ kind: "list" });
			}
			return;
		}
		if (mode.kind === "search") {
			if (isMouseEvent(char)) return; // clicks must not become search text
			const pane = mode.pane;
			const setQuery = (fn: (cur: string) => string) =>
				patchTab((s) => ({
					...s,
					search: { ...s.search, [pane]: fn(s.search[pane]) },
					selections: {
						...s.selections,
						[pane]: { cursor: 0, anchor: null },
					},
				}));
			if (key.return) {
				setMode({ kind: "list" });
			} else if (key.escape) {
				setQuery(() => "");
				setMode({ kind: "list" });
			} else if (key.backspace || key.delete) {
				setQuery((cur) => cur.slice(0, -1));
			} else if (char && !key.ctrl && !key.meta) {
				setQuery((cur) => cur + char);
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
		// Any remaining mouse report (click/release/motion) is not a keystroke:
		// swallow it so it never reaches handleKey and get mis-read as, e.g., the
		// digits in the coordinates triggering a tab switch.
		if (isMouseEvent(char)) return;

		setStatusLine(null);
		if (prefixTimer.current) {
			clearTimeout(prefixTimer.current);
			prefixTimer.current = null;
		}
		const keyInput: KeyInput = {
			input: char,
			ctrl: key.ctrl,
			shift: key.shift,
			upArrow: key.upArrow,
			downArrow: key.downArrow,
			leftArrow: key.leftArrow,
			rightArrow: key.rightArrow,
			return: key.return,
			escape: key.escape,
		};
		const result = handleKey(prefixArmed, ui.focus, keyInput);
		setPrefixArmed(result.prefixArmed);
		if (result.prefixArmed) {
			prefixTimer.current = setTimeout(() => setPrefixArmed(false), 2000);
		}
		if (result.action) dispatch(result.action);
	});

	const focusedRange =
		ui.focus === "detail" ? null : selectionRange(paneSel(ui.focus));
	const focusedSelectionCount =
		focusedRange === null || visibleCount(ui.focus as ListPaneId) === 0
			? 0
			: focusedRange.end - focusedRange.start + 1;

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
						rows={visibleQueueRows}
						selection={queueSel}
						focused={ui.focus === "queue"}
						capacity={queueCap}
						filter={ui.search.queue}
						filterActive={mode.kind === "search" && mode.pane === "queue"}
					/>
					<TasksPane
						defs={visibleDefs}
						selection={tasksSel}
						focused={ui.focus === "tasks"}
						capacity={listCap}
						filter={ui.search.tasks}
						filterActive={mode.kind === "search" && mode.pane === "tasks"}
					/>
					<WorktreesPane
						rows={visibleWtRows}
						selection={wtSel}
						focused={ui.focus === "worktrees"}
						capacity={listCap}
						filter={ui.search.worktrees}
						filterActive={mode.kind === "search" && mode.pane === "worktrees"}
					/>
				</Box>
				<DetailPane
					context={context}
					subTab={subTab}
					focused={ui.focus === "detail"}
					width={detailWidth}
					height={detailHeight}
					scrollOffset={ui.scrollOffset}
					runFiles={currentRunFiles}
					definition={detailDefinition}
				/>
			</Box>
			<Footer
				focus={ui.focus}
				prefixArmed={prefixArmed}
				statusLine={statusLine}
				searching={mode.kind === "search"}
				selectionCount={focusedSelectionCount}
			/>
			{mode.kind === "add-task" ? (
				<Modal
					title={`New task — ${mode.session} session — ${
						mode.worktree
							? `${activeName}:${stripRepoPrefix(mode.worktree, activeName ?? "")}`
							: `${activeName} (adhoc)`
					}`}
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
					title={`${mode.def.name} args`}
					columns={columns}
					rows={rows}
					hint="tab/↓ next · ←/→ cycle · enter run · esc cancel"
				>
					<ArgsForm
						args={mode.def.args}
						initial={mode.initial}
						fixed={mode.fixed}
						width={modalInner}
						onSubmit={(values) => {
							act(
								actions.runDefinition(
									mode.def.repo,
									mode.def.name,
									values,
									mode.worktree,
								),
							);
							invalidateDefs();
							setMode({ kind: "list" });
						}}
						onCancel={() => setMode({ kind: "list" })}
					/>
				</Modal>
			) : null}
			{mode.kind === "def-pick" ? (
				<Modal
					title={`Run task definition — ${
						mode.worktree
							? `${activeName}:${stripRepoPrefix(mode.worktree, activeName ?? "")}`
							: activeName
					}`}
					columns={columns}
					rows={rows}
					hint="↑/↓ move · enter run · q/esc close"
				>
					{mode.defs.map((def, i) => {
						const sel = i === mode.index;
						// Global defs carry a dimmed (g) marker; the 4-col slot keeps every
						// row's total width == modalInner so the modal stays opaque.
						const marker = def.scope === "global" ? " (g)" : "    ";
						const main = ` ${def.name}${
							def.args.length > 0 ? ` (${argSummary(def.args)})` : ""
						}${def.hasDiscovery ? " ⏰" : ""}`;
						return (
							<Box key={`${def.repo}/${def.name}`}>
								<Text inverse={sel}>{padLine(main, modalInner - 4)}</Text>
								<Text inverse={sel} dimColor>
									{marker}
								</Text>
							</Box>
						);
					})}
				</Modal>
			) : null}
			{mode.kind === "action-menu" ? (
				<Modal
					title={mode.title}
					columns={columns}
					rows={rows}
					hint="↑/↓ move · enter run · esc close"
				>
					{mode.items.map((item, i) => (
						<Text
							key={item.id}
							inverse={i === mode.index}
							dimColor={item.disabled !== undefined}
						>
							{padLine(
								` ${item.label}${item.disabled ? ` — ${item.disabled}` : ""}`,
								modalInner,
							)}
						</Text>
					))}
				</Modal>
			) : null}
			{mode.kind === "confirm-remove" ? (
				<Modal
					title={`Remove worktree — ${stripRepoPrefix(mode.worktree, activeName ?? "")}`}
					columns={columns}
					rows={rows}
					hint="y confirm · n/esc cancel"
				>
					<Text>
						{padLine(
							` wt remove ${mode.branch ?? stripRepoPrefix(mode.worktree, activeName ?? "")} — discards uncommitted changes`,
							modalInner,
						)}
					</Text>
					<Text>{padLine(` and deletes the local branch`, modalInner)}</Text>
				</Modal>
			) : null}
			{mode.kind === "create-worktree" ? (
				<Modal
					title={`Create worktree — ${activeName}`}
					columns={columns}
					rows={rows}
					hint="enter submit · esc cancel"
				>
					<TextInput
						label="branch"
						value={input}
						width={modalInner}
						onChange={setInput}
						onSubmit={(v) => {
							const invalid = validateBranchName(v);
							if (invalid !== null) {
								setMode({ kind: "create-worktree", error: invalid });
								return;
							}
							if (!activeName) return;
							// Close immediately — creation runs the repo's post-create
							// hooks and can take minutes; progress and the eventual
							// result live on the status line, not a blocked modal.
							setInput("");
							setMode({ kind: "list" });
							setStatusLine(`creating worktree ${v}…`);
							void actions.createWorktree(activeName, v).then((err) => {
								setStatusLine(
									err !== null ? `create worktree ${v}: ${err}` : null,
								);
							});
						}}
						onCancel={() => {
							setInput("");
							setMode({ kind: "list" });
						}}
					/>
					{mode.error ? (
						<Text color="red">{padLine(` ${mode.error}`, modalInner)}</Text>
					) : null}
				</Modal>
			) : null}
			{mode.kind === "confirm-bulk-remove" ? (
				<Modal
					title={`Remove ${mode.names.length} worktrees`}
					columns={columns}
					rows={rows}
					hint="y confirm · n/esc cancel"
				>
					<Text>
						{padLine(
							" discards uncommitted changes and deletes each local branch",
							modalInner,
						)}
					</Text>
					{mode.names.slice(0, 8).map((name) => (
						<Text key={name}>{padLine(`  ${name}`, modalInner)}</Text>
					))}
					{mode.names.length > 8 ? (
						<Text dimColor>
							{padLine(`  …and ${mode.names.length - 8} more`, modalInner)}
						</Text>
					) : null}
				</Modal>
			) : null}
		</Box>
	);
}
