import { Text } from "ink";
import type { PaneId } from "../keymap.js";

const HINTS: Record<PaneId, string> = {
	queue:
		"[C-s] prefix · [↑↓/jk] select · [r]etry · [s]kip · [w]orktree · [enter] detail · [q]uit",
	tasks: "[C-s] prefix · [↑↓/jk] select · [enter] run · [q]uit",
	worktrees:
		"[C-s] prefix · [↑↓/jk] select · [f]resh task · [m]ain task · [enter] run def · [q]uit",
	detail:
		"[C-s] prefix · [↑↓/jk] scroll · [g/G] top/bottom · [1-9] sub-tab · [q]uit",
};

const PREFIX_HINT = " PREFIX — arrows/hjkl move · 1-9 tab · n/p cycle ";

export function Footer({
	focus,
	prefixArmed,
	statusLine,
}: {
	focus: PaneId;
	prefixArmed: boolean;
	statusLine: string | null;
}) {
	if (statusLine !== null) return <Text color="red">{statusLine}</Text>;
	if (prefixArmed) return <Text inverse>{PREFIX_HINT}</Text>;
	return <Text dimColor>{HINTS[focus]}</Text>;
}
