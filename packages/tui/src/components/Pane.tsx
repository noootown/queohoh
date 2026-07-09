import { Box, Text } from "ink";
import type { ReactNode } from "react";

export function Pane({
	title,
	focused,
	children,
	flexGrow,
	flexBasis,
	height,
}: {
	title: string;
	focused: boolean;
	children: ReactNode;
	flexGrow?: number;
	flexBasis?: number;
	height?: number;
}) {
	return (
		<Box
			borderStyle="round"
			borderColor={focused ? "cyan" : "gray"}
			flexDirection="column"
			flexGrow={flexGrow}
			flexBasis={flexBasis}
			height={height}
			paddingX={1}
		>
			<Text bold>{title}</Text>
			{children}
		</Box>
	);
}
