import { Text } from "ink";
import type { PaneId } from "../keymap.js";

const LIST_HINT =
	"[C-s] prefix · [a] actions · [enter] detail · [↑↓] move · [/] filter · [q]uit";

const HINTS: Record<PaneId, string> = {
	queue: LIST_HINT,
	tasks: LIST_HINT,
	worktrees: LIST_HINT,
	detail:
		"[C-s] prefix · [↑↓/jk] scroll · [g/G] top/bottom · [1-9] sub-tab · [a] actions · [q]uit",
};

const PREFIX_HINT = " PREFIX — arrows/hjkl move · 1-9 tab · n/p cycle ";

export function Footer({
	focus,
	prefixArmed,
	statusLine,
	searching,
}: {
	focus: PaneId;
	prefixArmed: boolean;
	statusLine: string | null;
	searching: boolean;
}) {
	if (searching)
		return <Text dimColor>type to filter · [enter] apply · [esc] clear</Text>;
	if (statusLine !== null) return <Text color="red">{statusLine}</Text>;
	if (prefixArmed) return <Text inverse>{PREFIX_HINT}</Text>;
	return <Text dimColor>{HINTS[focus]}</Text>;
}
