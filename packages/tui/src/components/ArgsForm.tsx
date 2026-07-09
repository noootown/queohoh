import type { ArgSpec } from "@queohoh/core";
import { Box, Text, useInput } from "ink";
import { useRef, useState } from "react";
import { isMouseEvent } from "../keymap.js";
import { padLine } from "./Modal.js";

/** Whether an arg is an enum (has a non-empty `options` list). */
function isEnum(arg: ArgSpec): boolean {
	return arg.options !== undefined && arg.options.length > 0;
}

/**
 * Initial value for one arg: an explicit `initial` override wins, then the
 * declared `default`, then (for enums) the first option, else empty.
 */
function initialValue(arg: ArgSpec, initial?: Record<string, string>): string {
	const override = initial?.[arg.name];
	if (override !== undefined) return override;
	if (arg.default !== undefined) return arg.default;
	if (isEnum(arg) && arg.options) return arg.options[0] ?? "";
	return "";
}

/** The dimmed hint shown to the right of a row: options list or description. */
function rowHint(arg: ArgSpec): string {
	if (isEnum(arg) && arg.options) {
		const opts = arg.options.join(" | ");
		return arg.description ? `${opts} — ${arg.description}` : opts;
	}
	return arg.description ?? "";
}

/**
 * Per-arg form that replaces the single whitespace-split text box. One row per
 * arg: text args behave like `TextInput` (prefilled with the default, editable);
 * enum args render `‹value›` and cycle through `options` with ←/→. Focus moves
 * with tab/↓ (next) and shift-tab/↑ (previous), wrapping. Enter submits every
 * value positionally in arg order; a required (no-default) empty field blocks
 * submit with an inline red error on that row.
 */
export function ArgsForm({
	args,
	initial,
	fixed,
	width,
	onSubmit,
	onCancel,
}: {
	args: ArgSpec[];
	initial?: Record<string, string>;
	/**
	 * Values decided by the caller's context (e.g. squash-merge's `source` is
	 * the selected worktree's branch): rendered as dimmed read-only rows, never
	 * focusable or editable, but still submitted in positional order.
	 */
	fixed?: Record<string, string>;
	width: number;
	onSubmit: (values: string[]) => void;
	onCancel: () => void;
}) {
	const isFixed = (i: number): boolean => {
		const name = args[i]?.name;
		return name !== undefined && fixed?.[name] !== undefined;
	};
	const [values, setValues] = useState<string[]>(() =>
		args.map((a) => fixed?.[a.name] ?? initialValue(a, initial)),
	);
	const firstEditable = args.findIndex((_, i) => !isFixed(i));
	const [focus, setFocus] = useState(Math.max(0, firstEditable));
	const [errorRow, setErrorRow] = useState<number | null>(null);

	// Mirror controlled state into refs so a burst of synchronous keystrokes
	// (React batches them, leaving `values`/`focus` stale between them) still
	// accumulate correctly — the same pattern TextInput uses for its value.
	const valuesRef = useRef(values);
	valuesRef.current = values;
	const focusRef = useRef(focus);
	focusRef.current = focus;

	const count = args.length;
	const moveFocus = (delta: number) => {
		if (count === 0 || firstEditable === -1) return;
		// Step past fixed rows; bounded by count so an all-fixed form can't spin.
		let next = focusRef.current;
		for (let step = 0; step < count; step++) {
			next = (next + delta + count) % count;
			if (!isFixed(next)) break;
		}
		focusRef.current = next;
		setFocus(next);
	};
	const setValue = (idx: number, value: string) => {
		const next = valuesRef.current.slice();
		next[idx] = value;
		valuesRef.current = next;
		setValues(next);
	};

	useInput((input, key) => {
		// Mouse tracking is on while the modal is open; a stray click arrives as an
		// SGR report — drop it before any edit branch so it never lands in a field.
		if (isMouseEvent(input)) return;

		if (key.return) {
			// Block submit on the first required-and-empty field; flag that row.
			const missing = args.findIndex(
				(a, i) => a.default === undefined && valuesRef.current[i] === "",
			);
			if (missing !== -1) {
				if (!isFixed(missing)) {
					focusRef.current = missing;
					setFocus(missing);
				}
				setErrorRow(missing);
				return;
			}
			onSubmit(valuesRef.current.slice());
			return;
		}
		if (key.escape) {
			onCancel();
			return;
		}
		if (key.tab) {
			moveFocus(key.shift ? -1 : 1);
			return;
		}
		if (key.downArrow) {
			moveFocus(1);
			return;
		}
		if (key.upArrow) {
			moveFocus(-1);
			return;
		}

		const idx = focusRef.current;
		const arg = args[idx];
		if (!arg || isFixed(idx)) return;

		if (isEnum(arg) && arg.options) {
			// Enum rows: ←/→ cycle the options; typing is ignored.
			if (key.leftArrow || key.rightArrow) {
				const opts = arg.options;
				const cur = opts.indexOf(valuesRef.current[idx] ?? "");
				const base = cur === -1 ? 0 : cur;
				const step = key.rightArrow ? 1 : -1;
				const nextOpt = opts[(base + step + opts.length) % opts.length] ?? "";
				setValue(idx, nextOpt);
			}
			return;
		}

		// Text rows: TextInput-style editing (append / backspace).
		if (key.backspace || key.delete) {
			setValue(idx, (valuesRef.current[idx] ?? "").slice(0, -1));
			if (errorRow === idx) setErrorRow(null);
		} else if (input && !key.ctrl && !key.meta) {
			setValue(idx, (valuesRef.current[idx] ?? "") + input);
			if (errorRow === idx) setErrorRow(null);
		}
	});

	// Column split: the label+value column gets the majority, the dimmed
	// hint/error column fills the rest (both padded so every cell is opaque).
	const hintCol = Math.max(0, Math.min(Math.floor(width / 2), 40));
	const mainCol = Math.max(1, width - hintCol);
	const labelWidth = Math.max(0, ...args.map((a) => a.name.length));

	return (
		<>
			{args.map((arg, i) => {
				const rowFixed = isFixed(i);
				const focused = i === focus && !rowFixed;
				const value = values[i] ?? "";
				const label = padLine(`${arg.name}>`, labelWidth + 1);
				const shown = isEnum(arg) && !rowFixed ? `‹${value}›` : value;
				const cursor = focused ? "█" : "";
				const main = ` ${label} ${shown}${cursor}`;
				const hint =
					errorRow === i ? " required" : hintCol > 0 ? ` ${rowHint(arg)}` : "";
				return (
					<Box key={arg.name}>
						<Text inverse={focused} dimColor={rowFixed}>
							{padLine(main, mainCol)}
						</Text>
						{hintCol > 0 ? (
							<Text
								color={errorRow === i ? "red" : undefined}
								dimColor={errorRow !== i}
							>
								{padLine(hint, hintCol)}
							</Text>
						) : null}
					</Box>
				);
			})}
		</>
	);
}
