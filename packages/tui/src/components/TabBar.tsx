import { Box, Text } from "ink";
import type { ProjectTab } from "../selectors.js";

export function TabBar({
	tabs,
	activeIndex,
	connected,
	runningCount,
	maxConcurrent,
}: {
	tabs: ProjectTab[];
	activeIndex: number;
	connected: boolean;
	runningCount: number;
	maxConcurrent: number | null;
}) {
	const runLabel =
		maxConcurrent === null
			? `running ${runningCount}`
			: `running ${runningCount}/${maxConcurrent}`;
	return (
		<Box>
			<Box flexGrow={1}>
				{tabs.map((tab, i) => (
					<Text
						key={tab.name}
						bold={i === activeIndex}
						inverse={i === activeIndex}
					>
						{` ${i + 1}:${tab.name} `}
					</Text>
				))}
			</Box>
			{connected ? (
				<Text color="green">●</Text>
			) : (
				<Text color="yellow">daemon unreachable — retrying…</Text>
			)}
			<Text>{` ${runLabel}`}</Text>
		</Box>
	);
}
