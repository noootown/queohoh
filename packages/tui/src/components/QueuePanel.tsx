import { Text } from "ink";
import type { QueueRow } from "../format.js";
import { windowRows } from "../selectors.js";
import { Pane } from "./Pane.js";

export function QueuePane({
	rows,
	selectedIndex,
	focused,
	capacity,
}: {
	rows: QueueRow[];
	selectedIndex: number;
	focused: boolean;
	capacity: number;
}) {
	const { rows: windowed, offset } = windowRows(rows, selectedIndex, capacity);
	return (
		<Pane title="QUEUE" focused={focused} flexGrow={2} flexBasis={0}>
			{rows.length === 0 ? (
				<Text dimColor>queue empty — [f]/[m] on a worktree to add</Text>
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
