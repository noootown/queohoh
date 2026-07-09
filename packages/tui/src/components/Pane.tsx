import { Box, Text } from "ink";
import type { ReactNode } from "react";

export function Pane({
	title,
	focused,
	children,
	flexGrow,
	height,
}: {
	title: string;
	focused: boolean;
	children: ReactNode;
	flexGrow?: number;
	height?: number;
}) {
	return (
		<Box
			borderStyle="round"
			borderColor={focused ? "cyan" : "gray"}
			flexDirection="column"
			flexGrow={flexGrow}
			height={height}
			paddingX={1}
		>
			<Text bold>{title}</Text>
			{children}
		</Box>
	);
}
