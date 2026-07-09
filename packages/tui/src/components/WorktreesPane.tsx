import { Text } from "ink";
import { type WorktreeRow, windowRows } from "../selectors.js";
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
		<Pane title="WORKTREES" focused={focused} flexGrow={1}>
			{rows.length === 0 ? (
				<Text dimColor>no worktrees</Text>
			) : (
				windowed.map((row, i) => (
					<Text
						key={`${row.kind}:${row.path}`}
						inverse={offset + i === selectedIndex}
					>
						{row.name}{" "}
						{row.hasMainSession ? <Text color="cyan">◆ </Text> : null}
						{row.state === "you" ? (
							<Text color="yellow">YOU</Text>
						) : (
							<Text dimColor>{row.state}</Text>
						)}
					</Text>
				))
			)}
		</Pane>
	);
}
