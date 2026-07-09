import { Text } from "ink";
import type { DefinitionSummary } from "../actions.js";
import { windowRows } from "../selectors.js";
import { Pane } from "./Pane.js";

export function TasksPane({
	defs,
	selectedIndex,
	focused,
	capacity,
}: {
	defs: DefinitionSummary[];
	selectedIndex: number;
	focused: boolean;
	capacity: number;
}) {
	const { rows, offset } = windowRows(defs, selectedIndex, capacity);
	return (
		<Pane title="TASKS" focused={focused} flexGrow={1}>
			{defs.length === 0 ? (
				<Text dimColor>no task definitions</Text>
			) : (
				rows.map((def, i) => (
					<Text
						key={`${def.repo}/${def.name}`}
						inverse={offset + i === selectedIndex}
					>
						{def.name}
						{def.args.length > 0 ? ` (${def.args.join(", ")})` : ""}
						{def.hasDiscovery ? " ⏰" : ""}
					</Text>
				))
			)}
		</Pane>
	);
}
