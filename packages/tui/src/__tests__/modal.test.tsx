import { Box, Text } from "ink";
import { render } from "ink-testing-library";
import type { ReactNode } from "react";
import { describe, expect, it } from "vitest";
import {
	Modal,
	modalGeometry,
	modalInnerWidth,
	padLine,
} from "../components/Modal.js";
import { TextInput } from "../components/TextInput.js";

// Modal is absolute-positioned, so it only renders inside a sized
// `position="relative"` root (the real App usage). This mirrors that.
function renderInRoot(
	cols: number,
	rows: number,
	node: ReactNode,
	body?: ReactNode,
) {
	return render(
		<Box width={cols} height={rows} flexDirection="column" position="relative">
			{body}
			{node}
		</Box>,
	);
}

describe("modalGeometry", () => {
	it("clamps width to 72 on wide terminals and centers it", () => {
		const g = modalGeometry(200, 40, 4);
		expect(g.width).toBe(72);
		expect(g.marginLeft).toBe(64); // (200 - 72) / 2
	});

	it("tracks columns - 8 for mid-width terminals", () => {
		const g = modalGeometry(50, 40, 4);
		expect(g.width).toBe(42); // 50 - 8
		expect(g.marginLeft).toBe(4); // (50 - 42) / 2
	});

	it("floors width at 20 for very narrow terminals", () => {
		const g = modalGeometry(25, 40, 4);
		expect(g.width).toBe(20); // 25 - 8 = 17 -> floored to 20
		expect(g.marginLeft).toBe(2); // floor((25 - 20) / 2)
	});

	it("centers vertically from content height + border rows and floors", () => {
		const g = modalGeometry(80, 20, 4);
		// outer height = contentHeight(4) + border(2) = 6; (20 - 6) / 2 = 7
		expect(g.marginTop).toBe(7);
	});

	it("uses floor division for odd horizontal gaps", () => {
		const g = modalGeometry(41, 40, 4);
		expect(g.width).toBe(33); // 41 - 8
		expect(g.marginLeft).toBe(4); // floor((41 - 33) / 2) = 4
	});

	it("never returns negative offsets when the modal fills the screen", () => {
		const g = modalGeometry(20, 4, 10);
		expect(g.marginLeft).toBeGreaterThanOrEqual(0);
		expect(g.marginTop).toBeGreaterThanOrEqual(0);
	});
});

describe("padLine", () => {
	it("pads short text to the requested width with spaces", () => {
		expect(padLine("hi", 6)).toBe("hi    ");
		expect(padLine("hi", 6)).toHaveLength(6);
	});

	it("returns text unchanged when already at width", () => {
		expect(padLine("abcdef", 6)).toBe("abcdef");
	});

	it("truncates text longer than width", () => {
		expect(padLine("abcdefgh", 6)).toBe("abcdef");
	});
});

describe("modalInnerWidth", () => {
	it("subtracts the two border columns (padding baked into text)", () => {
		expect(modalInnerWidth(32)).toBe(30);
	});
});

describe("Modal", () => {
	it("renders the title and dim hint line", () => {
		const { lastFrame } = renderInRoot(
			60,
			20,
			<Modal title="Add task" columns={60} rows={20} hint="esc close">
				<Text>body</Text>
			</Modal>,
		);
		const frame = lastFrame() ?? "";
		expect(frame).toContain("Add task");
		expect(frame).toContain("esc close");
	});

	it("renders children inside the modal", () => {
		const { lastFrame } = renderInRoot(
			60,
			20,
			<Modal title="Pick" columns={60} rows={20} hint="esc">
				<Text>option-one</Text>
			</Modal>,
		);
		expect(lastFrame() ?? "").toContain("option-one");
	});

	it("pads the title line to the full inner width so it is opaque", () => {
		const { lastFrame } = renderInRoot(
			40,
			20,
			<Modal title="Hi" columns={40} rows={20} hint="esc">
				<Text>x</Text>
			</Modal>,
		);
		const frame = lastFrame() ?? "";
		const titleRow = frame.split("\n").find((l) => l.includes("Hi")) ?? "";
		// width = 40 - 8 = 32; inner = 28. "Hi" is followed by a run of spaces
		// (then padding + right border), so trailing space is preserved.
		expect(titleRow).toMatch(/Hi\s{20,}/);
	});

	it("composites over body text: body outside visible, interior opaque", () => {
		const cols = 60;
		const rows = 16;
		const fill = "X".repeat(cols);
		const body = Array.from({ length: rows }, (_, i) => (
			// biome-ignore lint/suspicious/noArrayIndexKey: static fixture
			<Text key={i}>{fill}</Text>
		));
		const inner = modalInnerWidth(modalGeometry(cols, rows, 3).width);
		const { lastFrame } = renderInRoot(
			cols,
			rows,
			<Modal title="TITLE" columns={cols} rows={rows} hint="esc close">
				<Text>{padLine("content-line", inner)}</Text>
			</Modal>,
			body,
		);
		const frame = lastFrame() ?? "";
		const lines = frame.split("\n");
		// (c) top body row untouched.
		expect(lines[0]).toContain("XXXXXXXXXX");
		// (a) a modal border corner appears (absolute offset applied).
		expect(frame).toMatch(/[╭╮╰╯]/);
		// (b) interior content row overwrites body: no X between the content text
		// and the right border (self-padded child is fully opaque).
		const contentRow = lines.find((l) => l.includes("content-line")) ?? "";
		const idx = contentRow.indexOf("content-line");
		const rightBorder = contentRow.indexOf("│", idx);
		const between = contentRow.slice(idx + "content-line".length, rightBorder);
		expect(between).not.toContain("X");
		// (d) the title and hint rows are equally opaque: no body X survives
		// between the left and right borders of either row.
		for (const marker of ["TITLE", "esc close"]) {
			const row = lines.find((l) => l.includes(marker)) ?? "";
			const left = row.indexOf("│");
			const right = row.indexOf("│", left + 1);
			expect(left).toBeGreaterThanOrEqual(0);
			expect(right).toBeGreaterThan(left);
			expect(row.slice(left + 1, right)).not.toContain("X");
		}
		// (e) compositing leaves the body visible outside the modal: X survives to
		// both the LEFT and RIGHT of the modal on a modal-spanned row.
		const spannedRow = lines.find((l) => l.includes("TITLE")) ?? "";
		const leftBorder = spannedRow.indexOf("│");
		const rightBorderEdge = spannedRow.lastIndexOf("│");
		expect(spannedRow.slice(0, leftBorder)).toContain("X");
		expect(spannedRow.slice(rightBorderEdge + 1)).toContain("X");
	});
});

describe("TextInput composed inside Modal", () => {
	it("is opaque over body text when given the modal inner width", () => {
		const cols = 60;
		const rows = 16;
		const fill = "X".repeat(cols);
		const body = Array.from({ length: rows }, (_, i) => (
			// biome-ignore lint/suspicious/noArrayIndexKey: static fixture
			<Text key={i}>{fill}</Text>
		));
		const inner = modalInnerWidth(modalGeometry(cols, rows, 3).width);
		const { lastFrame } = renderInRoot(
			cols,
			rows,
			<Modal title="Add" columns={cols} rows={rows} hint="esc close">
				<TextInput
					label="prompt"
					value="hi"
					width={inner}
					onChange={() => {}}
					onSubmit={() => {}}
					onCancel={() => {}}
				/>
			</Modal>,
			body,
		);
		const frame = lastFrame() ?? "";
		const line = frame.split("\n").find((l) => l.includes("prompt")) ?? "";
		expect(line).toContain("prompt> hi");
		const idx = line.indexOf("prompt");
		const rightBorder = line.indexOf("│", idx);
		const between = line.slice(idx, rightBorder);
		expect(between).not.toContain("X");
	});
});
