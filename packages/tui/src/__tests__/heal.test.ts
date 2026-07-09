import { describe, expect, it } from "vitest";
import { decideHeal, isStale } from "../heal.js";

describe("isStale", () => {
	it("is false when the daemon build matches disk", () => {
		expect(isStale("100", "100")).toBe(false);
	});

	it("is true when the daemon build differs from disk", () => {
		expect(isStale("100", "200")).toBe(true);
	});

	it("treats a pre-feature daemon (undefined buildId) as stale", () => {
		expect(isStale(undefined, "200")).toBe(true);
	});
});

describe("decideHeal", () => {
	const base = {
		snapshotBuildId: "100" as string | undefined,
		diskBuildId: "100",
		runningCount: 0,
		lastHealedBuildId: null as string | null,
	};

	it("does nothing when up to date", () => {
		expect(decideHeal(base)).toBe("none");
	});

	it("restarts now when stale and idle", () => {
		expect(
			decideHeal({ ...base, snapshotBuildId: "100", diskBuildId: "200" }),
		).toBe("restart-now");
	});

	it("defers when stale but a task is running", () => {
		expect(
			decideHeal({
				...base,
				snapshotBuildId: "100",
				diskBuildId: "200",
				runningCount: 1,
			}),
		).toBe("defer");
	});

	it("treats a pre-feature daemon (undefined) as stale and restarts when idle", () => {
		expect(
			decideHeal({ ...base, snapshotBuildId: undefined, diskBuildId: "200" }),
		).toBe("restart-now");
	});

	it("does not retry a disk build it already attempted (loop guard)", () => {
		expect(
			decideHeal({
				...base,
				snapshotBuildId: "100",
				diskBuildId: "200",
				lastHealedBuildId: "200", // already tried to reach 200
			}),
		).toBe("none");
	});

	it("re-heals when a fresh build lands after a prior attempt", () => {
		// Tried to reach 200 before; disk has since advanced to 300 — a new mismatch.
		expect(
			decideHeal({
				...base,
				snapshotBuildId: "100",
				diskBuildId: "300",
				lastHealedBuildId: "200",
			}),
		).toBe("restart-now");
	});

	it("running takes priority over nothing, but the loop guard still wins", () => {
		// Even with a task running, an already-attempted build stays 'none'.
		expect(
			decideHeal({
				...base,
				snapshotBuildId: "100",
				diskBuildId: "200",
				runningCount: 2,
				lastHealedBuildId: "200",
			}),
		).toBe("none");
	});
});
