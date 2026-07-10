import { Text } from "ink";
import { memo } from "react";
import type { QueueRow } from "../format.js";
import {
	type PaneSelection,
	paneTitle,
	selectionRange,
	windowRows,
} from "../selectors.js";
import { Pane } from "./Pane.js";

// Memoized: all props are primitives or the memoized `rows` array from App, so a
// render triggered by unrelated state (now-tick, modal toggle) skips this pane.
export const QueuePane = memo(function QueuePane({
	rows,
	selection,
	focused,
	capacity,
	filter,
	filterActive,
}: {
	rows: QueueRow[];
	selection: PaneSelection;
	focused: boolean;
	capacity: number;
	filter: string;
	filterActive: boolean;
}) {
	const { start, end } = selectionRange(selection);
	const selectedCount = rows.length === 0 ? 0 : end - start + 1;
	const { rows: windowed, offset } = windowRows(
		rows,
		selection.cursor,
		capacity,
	);
	return (
		<Pane
			title={paneTitle("QUEUE", filter, filterActive, selectedCount)}
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
						inverse={focused && offset + i >= start && offset + i <= end}
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
});
