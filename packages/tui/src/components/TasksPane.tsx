import { Text } from "ink";
import { memo } from "react";
import { argSummary, type DefinitionSummary } from "../actions.js";
import {
	type PaneSelection,
	paneTitle,
	selectionRange,
	windowRows,
} from "../selectors.js";
import { Pane } from "./Pane.js";

// Memoized: props are primitives or the memoized `defs` array from App.
export const TasksPane = memo(function TasksPane({
	defs,
	selection,
	focused,
	capacity,
	filter,
	filterActive,
}: {
	defs: DefinitionSummary[];
	selection: PaneSelection;
	focused: boolean;
	capacity: number;
	filter: string;
	filterActive: boolean;
}) {
	const { start, end } = selectionRange(selection);
	const selectedCount = defs.length === 0 ? 0 : end - start + 1;
	const { rows, offset } = windowRows(defs, selection.cursor, capacity);
	return (
		<Pane
			title={paneTitle("TASKS", filter, filterActive, selectedCount)}
			focused={focused}
			flexGrow={1}
			flexBasis={0}
		>
			{defs.length === 0 ? (
				<Text dimColor>no task definitions</Text>
			) : (
				rows.map((def, i) => (
					<Text
						key={`${def.repo}/${def.name}`}
						inverse={focused && offset + i >= start && offset + i <= end}
						wrap="truncate"
					>
						{def.name}
						{def.args.length > 0 ? ` (${argSummary(def.args)})` : ""}
						{def.hasDiscovery ? " ⏰" : ""}
					</Text>
				))
			)}
		</Pane>
	);
});
