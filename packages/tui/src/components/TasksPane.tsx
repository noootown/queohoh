import { Text } from "ink";
import { memo } from "react";
import { argSummary, type DefinitionSummary } from "../actions.js";
import { paneTitle, windowRows } from "../selectors.js";
import { Pane } from "./Pane.js";

// Memoized: props are primitives or the memoized `defs` array from App.
export const TasksPane = memo(function TasksPane({
	defs,
	selectedIndex,
	focused,
	capacity,
	filter,
	filterActive,
}: {
	defs: DefinitionSummary[];
	selectedIndex: number;
	focused: boolean;
	capacity: number;
	filter: string;
	filterActive: boolean;
}) {
	const { rows, offset } = windowRows(defs, selectedIndex, capacity);
	return (
		<Pane
			title={paneTitle("TASKS", filter, filterActive)}
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
						inverse={focused && offset + i === selectedIndex}
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
