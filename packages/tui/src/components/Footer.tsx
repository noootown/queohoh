import { Text } from "ink";
import { memo } from "react";
import type { PaneId } from "../keymap.js";

const LIST_HINT =
	"[C-s] prefix · [a] actions · [enter] detail · [↑↓] move · [/] filter · [q]uit";

const HINTS: Record<PaneId, string> = {
	queue: `[c] new run · ${LIST_HINT}`,
	tasks: LIST_HINT,
	worktrees: `[c] new worktree · ${LIST_HINT}`,
	detail:
		"[C-s] prefix · [↑↓/jk] scroll · [g/G] top/bottom · [1-9] sub-tab · [a] actions · [q]uit",
};

const PREFIX_HINT = " PREFIX — arrows/hjkl move · 1-9 tab · n/p cycle ";

// Memoized: all props are primitives.
export const Footer = memo(function Footer({
	focus,
	prefixArmed,
	statusLine,
	searching,
	selectionCount,
}: {
	focus: PaneId;
	prefixArmed: boolean;
	statusLine: string | null;
	searching: boolean;
	selectionCount: number;
}) {
	if (searching)
		return <Text dimColor>type to filter · [enter] apply · [esc] clear</Text>;
	if (statusLine !== null) return <Text color="red">{statusLine}</Text>;
	if (prefixArmed) return <Text inverse>{PREFIX_HINT}</Text>;
	if (selectionCount > 1)
		return (
			<Text dimColor>
				{selectionCount} selected · [a] bulk actions · [shift+↑↓] extend · [esc]
				clear
			</Text>
		);
	return <Text dimColor>{HINTS[focus]}</Text>;
});
