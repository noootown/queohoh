import { Text } from "ink";
import type { QueueRow } from "../format.js";
import { paneTitle, windowRows } from "../selectors.js";
import { Pane } from "./Pane.js";

export function QueuePane({
	rows,
	selectedIndex,
	focused,
	capacity,
	filter,
	filterActive,
}: {
	rows: QueueRow[];
	selectedIndex: number;
	focused: boolean;
	capacity: number;
	filter: string;
	filterActive: boolean;
}) {
	const { rows: windowed, offset } = windowRows(rows, selectedIndex, capacity);
	return (
		<Pane
			title={paneTitle("QUEUE", filter, filterActive)}
			focused={focused}
			flexGrow={2}
			flexBasis={0}
		>
			{rows.length === 0 ? (
				<Text dimColor>queue empty — [a] on a worktree to add a task</Text>
			) : (
				windowed.map((row, i) => (
					<Text
						key={row.id + row.kind}
						inverse={focused && offset + i === selectedIndex}
						dimColor={row.kind === "archived"}
						wrap="truncate"
					>
						{row.glyph} {row.sessionMarker}
						{row.lane} {row.summary} {row.detail}
					</Text>
				))
			)}
		</Pane>
	);
}
