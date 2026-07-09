import { Box, Text } from "ink";
import { Children, type JSX, type ReactNode } from "react";

/**
 * Pure geometry for a centered floating modal.
 *
 * Width tracks `columns - 8`, capped at 72 and floored at 20. The modal is
 * centered horizontally and vertically (floor division); offsets never go
 * negative even when the modal is larger than the terminal. `contentHeight` is
 * the interior line count (title + children + hint); border rows are added here.
 */
export function modalGeometry(
	columns: number,
	rows: number,
	contentHeight: number,
): { width: number; marginLeft: number; marginTop: number } {
	const width = Math.max(20, Math.min(72, columns - 8));
	const marginLeft = Math.max(0, Math.floor((columns - width) / 2));
	const outerHeight = contentHeight + 2; // top + bottom border rows
	const marginTop = Math.max(0, Math.floor((rows - outerHeight) / 2));
	return { width, marginLeft, marginTop };
}

/**
 * Pad `text` with trailing spaces to exactly `width` columns, truncating if it
 * is longer. Used to make modal interior lines opaque: because Ink Boxes have
 * no background fill, a short line would let underlying body text bleed through
 * the transparent cells. Exported for reuse by picker rows.
 */
export function padLine(text: string, width: number): string {
	if (text.length >= width) return text.slice(0, width);
	return text + " ".repeat(width - text.length);
}

/**
 * Inner content width available for a modal of the given outer `width`:
 * just the two border columns are reserved. The modal uses `paddingX={0}`
 * on purpose: Ink padding cells are transparent (no background fill), so a
 * padded gutter would let underlying body text bleed through. Instead the
 * whole content region is filled by padded text (see `Modal`), and callers
 * pad their rows to this width so every interior cell is opaque.
 */
export function modalInnerWidth(width: number): number {
	return Math.max(1, width - 2);
}

/**
 * A centered floating modal (nvim/telescope style). Rendered by `App` as the
 * last child of a `position="relative"` root so Ink paints it over the body.
 *
 * SPIKE OUTCOME: compositing works. Ink 6.8 applies the absolute `marginLeft`/
 * `marginTop` offsets and paints this later sibling's border + padded interior
 * cells over the earlier body siblings, while body text outside the modal
 * bounds stays visible. No fallback needed. See task-5-report.md for the exact
 * frame evidence.
 */
export function Modal({
	title,
	columns,
	rows,
	hint,
	children,
}: {
	title: string;
	columns: number;
	rows: number;
	hint: string;
	children: ReactNode;
}): JSX.Element {
	// Interior line count: title + children lines + hint. Children are expected
	// to be one line each (a TextInput, or picker rows); this drives vertical
	// centering only, so a rough count is fine.
	const contentHeight = Children.count(children) + 2;
	const { width, marginLeft, marginTop } = modalGeometry(
		columns,
		rows,
		contentHeight,
	);
	const inner = modalInnerWidth(width);
	// paddingX={0}: pad text edge-to-edge (with a leading-space gutter baked in)
	// so every interior cell is opaque. Ink padding cells would be transparent.
	return (
		<Box
			position="absolute"
			marginLeft={marginLeft}
			marginTop={marginTop}
			width={width}
			borderStyle="round"
			borderColor="cyan"
			flexDirection="column"
			paddingX={0}
		>
			<Text bold>{padLine(` ${title}`, inner)}</Text>
			{children}
			<Text dimColor>{padLine(` ${hint}`, inner)}</Text>
		</Box>
	);
}
