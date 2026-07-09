import { Text, useInput } from "ink";
import { useRef } from "react";
import { isMouseEvent } from "../keymap.js";
import { padLine } from "./Modal.js";

export function TextInput({
	label,
	value,
	onChange,
	onSubmit,
	onCancel,
	width,
}: {
	label: string;
	value: string;
	onChange: (value: string) => void;
	onSubmit: (value: string) => void;
	onCancel: () => void;
	/**
	 * When composed inside a Modal, the interior line must be padded to the
	 * modal's inner width so it overwrites the body text beneath (Ink Boxes have
	 * no background fill). Omit for the legacy full-width footer input.
	 */
	width?: number;
}) {
	// Sync a ref to the controlled value on every render. Rapid synchronous
	// keystrokes (e.g. a for-loop of stdin writes in tests) are batched by React,
	// so the `value` prop is stale between them; the ref accumulates correctly
	// because it is mutated synchronously and only re-synced when a render lands.
	const valueRef = useRef(value);
	valueRef.current = value;
	useInput((input, key) => {
		// Mouse tracking is on while a modal is open, so a stray click arrives here
		// as an SGR report (e.g. `[<0;34;12M`). Drop it before the append branch so
		// the garbage never lands in the field value.
		if (isMouseEvent(input)) return;
		if (key.return) {
			onSubmit(valueRef.current);
		} else if (key.escape) {
			onCancel();
		} else if (key.backspace || key.delete) {
			valueRef.current = valueRef.current.slice(0, -1);
			onChange(valueRef.current);
		} else if (input && !key.ctrl && !key.meta) {
			valueRef.current += input;
			onChange(valueRef.current);
		}
	});
	// Leading-space gutter matches the modal title/hint/picker rows so the input
	// aligns with them instead of sitting flush against the border. padLine keeps
	// the total width at the provided `width`.
	const line = ` ${label}> ${value}█`;
	return <Text>{width !== undefined ? padLine(line, width) : line}</Text>;
}
