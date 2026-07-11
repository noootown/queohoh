import type { TaskInstance } from "./task.js";
import { laneKey } from "./task.js";

export interface LiveState {
	runningLanes: Set<string>;
	interactiveLanes: Set<string>;
	runningCount: number;
}

export interface ScheduleDecision {
	start: TaskInstance[];
	resolve: TaskInstance[];
	/** Chain members whose predecessor did not succeed — the engine marks each
	 * `skipped` with the given reason. Not resource-limited (independent of the
	 * concurrency cap). */
	skip: { task: TaskInstance; reason: string }[];
}

const PRIORITY_ORDER = { high: 0, normal: 1, low: 2 } as const;

/** True for a chain member that has a predecessor (head, seq 0, has none). */
function isChainTail(t: TaskInstance): boolean {
	return t.chainId != null && typeof t.chainSeq === "number" && t.chainSeq > 0;
}

/**
 * Statuses that mean a chain predecessor will never succeed, so its successor is
 * skipped rather than left waiting: an outright failure, a user cancel (stop →
 * cancelled, or a queued/needs-input skip → cancelled), a predecessor already
 * skipped (cascade), or a predecessor parked in needs-input (its shared worktree
 * never resolved, so the chain can't proceed).
 */
function isTerminalNonSuccess(status: TaskInstance["status"]): boolean {
	return (
		status === "failed" ||
		status === "needs-input" ||
		status === "skipped" ||
		status === "cancelled"
	);
}

export function schedule(
	tasks: TaskInstance[],
	live: LiveState,
	opts: { maxConcurrent: number },
): ScheduleDecision {
	const eligible = tasks
		.filter((t) => t.status === "queued")
		.sort((a, b) => {
			const band = PRIORITY_ORDER[a.priority] - PRIORITY_ORDER[b.priority];
			return band !== 0 ? band : a.id.localeCompare(b.id);
		});

	const start: TaskInstance[] = [];
	const resolve: TaskInstance[] = [];
	const skip: { task: TaskInstance; reason: string }[] = [];
	// Members skipped earlier in THIS pass, so a multi-step chain cascades in one
	// tick (a member skipped here is treated as a failed predecessor below).
	const skippedThisPass = new Set<string>();
	const claimedLanes = new Set<string>();
	let slots = opts.maxConcurrent - live.runningCount;

	for (const t of eligible) {
		// Chain ordering gate (members after the head). Independent tasks and the
		// head (seq 0) fall straight through to the normal lane logic below.
		if (isChainTail(t)) {
			const pred = tasks.find(
				(p) => p.chainId === t.chainId && p.chainSeq === (t.chainSeq ?? 0) - 1,
			);
			const predFailed =
				pred === undefined ||
				isTerminalNonSuccess(pred.status) ||
				skippedThisPass.has(pred.id);
			if (pred?.status !== "done") {
				if (predFailed) {
					skip.push({
						task: t,
						reason: `skipped: chain predecessor ${pred ? pred.status : "missing"}`,
					});
					skippedThisPass.add(t.id);
				}
				// else predecessor still queued/running → wait for a later tick.
				continue;
			}
			// Predecessor succeeded, but the tail must never resolve its own ref
			// (that would spawn a SECOND worktree for a `temp` chain). Its worktree
			// is stamped by the engine when the head resolves; until then, wait.
			if (t.target.worktree === null) continue;
		}

		if (slots <= 0) continue; // cap reached — keep scanning so skips still land
		const lane = laneKey(t);
		if (lane === null) {
			resolve.push(t);
			slots -= 1;
			continue;
		}
		// Remaining lane gates: per-lane serialization against tasks already
		// running (`runningLanes`) and one-start-per-lane-per-tick
		// (`claimedLanes`). Failed tasks no longer pause their lane, and an
		// active interactive/main session no longer holds its lane.
		if (live.runningLanes.has(lane) || claimedLanes.has(lane)) {
			continue;
		}
		start.push(t);
		claimedLanes.add(lane);
		slots -= 1;
	}

	return { start, resolve, skip };
}
