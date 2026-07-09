import { Text } from "ink";
import {
	type WorktreeRow,
	windowRows,
	worktreeDotColor,
} from "../selectors.js";
import { Pane } from "./Pane.js";

export function WorktreesPane({
	rows,
	selectedIndex,
	focused,
	capacity,
}: {
	rows: WorktreeRow[];
	selectedIndex: number;
	focused: boolean;
	capacity: number;
}) {
	const { rows: windowed, offset } = windowRows(rows, selectedIndex, capacity);
	return (
		<Pane title="WORKTREES" focused={focused} flexGrow={1} flexBasis={0}>
			{rows.length === 0 ? (
				<Text dimColor>no worktrees</Text>
			) : (
				windowed.map((row, i) => (
					<Text
						key={`${row.kind}:${row.path}`}
						inverse={focused && offset + i === selectedIndex}
						wrap="truncate"
					>
						<Text color={worktreeDotColor(row.state)}>●</Text> {row.name}
						{row.hasMainSession ? <Text color="cyan"> ◆</Text> : null}
						{row.queued > 0 ? <Text dimColor> [{row.queued}]</Text> : null}
					</Text>
				))
			)}
		</Pane>
	);
}
