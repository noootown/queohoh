import type { TaskDefinition } from "@queohoh/core";
import { Box, Text } from "ink";
import {
	anchorFor,
	type DetailContext,
	subTabsFor,
	windowLines,
} from "../detail.js";
import { promptSummary, statusGlyph } from "../format.js";

interface ContentView {
	lines: string[];
	anchor: "top" | "bottom";
	/** Shown when `lines` is empty. */
	placeholder: string;
}

function configLines(def: TaskDefinition): string[] {
	return [
		`args: ${def.args.length > 0 ? def.args.join(", ") : "—"}`,
		`worktree: ${def.worktree}`,
		`dedup: ${def.dedup}`,
		`model: ${def.model}`,
		`timeout: ${def.timeoutMs}ms`,
		`priority: ${def.priority}`,
		`discovery: ${def.discovery ? def.discovery.command : "—"}`,
	];
}

function contentFor(
	context: DetailContext,
	subTab: number,
	width: number,
	runFiles: { report: string | null; transcriptTail: string[] } | null,
	definition: TaskDefinition | null,
): ContentView {
	const anchor = anchorFor(context.kind, subTab);
	switch (context.kind) {
		case "run": {
			if (subTab === 1) {
				const report = runFiles?.report;
				return {
					lines: report ? report.split("\n") : [],
					anchor,
					placeholder: "(no report yet)",
				};
			}
			if (subTab === 2) {
				return {
					lines: context.task.prompt.split("\n"),
					anchor,
					placeholder: "(no prompt)",
				};
			}
			return {
				lines: runFiles?.transcriptTail ?? [],
				anchor,
				placeholder: "(no transcript yet)",
			};
		}
		case "definition": {
			if (definition === null) {
				return {
					lines: [],
					anchor,
					placeholder: "(loading definition…)",
				};
			}
			if (subTab === 1) {
				return {
					lines: configLines(definition),
					anchor,
					placeholder: "",
				};
			}
			return {
				lines: definition.prompt.split("\n"),
				anchor,
				placeholder: "(no prompt)",
			};
		}
		case "worktree": {
			const { row, laneTasks } = context;
			const lines = [
				`path: ${row.path}`,
				`branch: ${row.branch ?? "—"}`,
				`state: ${row.state}`,
				"",
				"tasks on this lane:",
			];
			if (laneTasks.length === 0) {
				lines.push("(none)");
			} else {
				for (const task of laneTasks) {
					lines.push(
						`${statusGlyph(task.status)} ${promptSummary(task.prompt, width)}`,
					);
				}
			}
			return { lines, anchor, placeholder: "" };
		}
		default:
			return { lines: [], anchor, placeholder: "(nothing selected)" };
	}
}

export function DetailPane({
	context,
	subTab,
	focused,
	width,
	height,
	scrollOffset,
	runFiles,
	definition,
}: {
	context: DetailContext;
	subTab: number;
	focused: boolean;
	width: number;
	height: number;
	scrollOffset: number;
	runFiles: { report: string | null; transcriptTail: string[] } | null;
	definition: TaskDefinition | null;
}) {
	const tabs = subTabsFor(context.kind);
	const view = contentFor(context, subTab, width, runFiles, definition);
	const visible =
		view.lines.length === 0
			? []
			: windowLines(view.lines, height, scrollOffset, view.anchor);

	return (
		<Box
			borderStyle="round"
			borderColor={focused ? "cyan" : "gray"}
			flexDirection="column"
			flexGrow={1}
			paddingX={1}
		>
			<Text bold>DETAIL</Text>
			{tabs.length > 0 ? (
				<Box>
					{tabs.map((label, i) => (
						<Text key={label} inverse={i === subTab} bold={i === subTab}>
							{` ${i + 1}:${label} `}
						</Text>
					))}
				</Box>
			) : null}
			{view.lines.length === 0 ? (
				<Text dimColor>{view.placeholder}</Text>
			) : (
				visible.map((line, i) => (
					// biome-ignore lint/suspicious/noArrayIndexKey: windowed lines have no stable identity; the window fully re-renders each refresh
					<Text key={`${i}-${line.slice(0, 8)}`}>
						{line === "" ? " " : line}
					</Text>
				))
			)}
		</Box>
	);
}
