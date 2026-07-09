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
}

const PRIORITY_ORDER = { high: 0, normal: 1, low: 2 } as const;

export function schedule(
	tasks: TaskInstance[],
	live: LiveState,
	opts: { maxConcurrent: number },
): ScheduleDecision {
	const pausedLanes = new Set<string>();
	for (const t of tasks) {
		if (t.status === "failed") {
			const lane = laneKey(t);
			if (lane) pausedLanes.add(lane);
		}
	}

	const eligible = tasks
		.filter((t) => t.status === "queued")
		.sort((a, b) => {
			const band = PRIORITY_ORDER[a.priority] - PRIORITY_ORDER[b.priority];
			return band !== 0 ? band : a.id.localeCompare(b.id);
		});

	const start: TaskInstance[] = [];
	const resolve: TaskInstance[] = [];
	const claimedLanes = new Set<string>();
	let slots = opts.maxConcurrent - live.runningCount;

	for (const t of eligible) {
		if (slots <= 0) break;
		const lane = laneKey(t);
		if (lane === null) {
			resolve.push(t);
			slots -= 1;
			continue;
		}
		if (
			live.runningLanes.has(lane) ||
			live.interactiveLanes.has(lane) ||
			pausedLanes.has(lane) ||
			claimedLanes.has(lane)
		) {
			continue;
		}
		start.push(t);
		claimedLanes.add(lane);
		slots -= 1;
	}

	return { start, resolve };
}
