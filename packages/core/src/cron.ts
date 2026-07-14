/**
 * Pure 5-field cron: `minute hour day-of-month month day-of-week`. No I/O and no
 * wall-clock reads — time enters only as an explicit `Date`/`nowMs` argument, so
 * every function is deterministically testable. Evaluated in LOCAL time (the
 * migrated `30 15 * * *` means 15:30 in the operator's timezone).
 *
 * Supports `*`, a number, comma lists (`1,15`), ranges (`1-5`), and steps on a
 * star or range (a star-step like every-15, or `0-30/10`). Month/weekday NAMES
 * are intentionally not supported in slice 2 — a name throws rather than
 * silently mis-scheduling.
 */
export interface CronSpec {
	minute: Set<number>; // 0-59
	hour: Set<number>; // 0-23
	dom: Set<number>; // 1-31
	month: Set<number>; // 1-12
	dow: Set<number>; // 0-6 (Sunday = 0)
	/** The day-of-month field was not `*` (drives dom/dow OR-semantics). */
	domRestricted: boolean;
	/** The day-of-week field was not `*`. */
	dowRestricted: boolean;
}

/** Expand one field into the set of integers it permits. `isDow` folds 7 → 0. */
function parseField(
	raw: string,
	lo: number,
	hi: number,
	isDow: boolean,
): Set<number> {
	const out = new Set<number>();
	for (const part of raw.split(",")) {
		const slash = part.indexOf("/");
		const rangePart = slash === -1 ? part : part.slice(0, slash);
		const stepStr = slash === -1 ? undefined : part.slice(slash + 1);
		const step = stepStr === undefined ? 1 : Number(stepStr);
		if (!Number.isInteger(step) || step < 1) {
			throw new Error(`cron: bad step in "${part}"`);
		}
		let start: number;
		let end: number;
		if (rangePart === "*") {
			start = lo;
			end = hi;
		} else if (rangePart.includes("-")) {
			const [a, b] = rangePart.split("-");
			start = Number(a);
			end = Number(b);
		} else {
			start = Number(rangePart);
			// A bare number with a step (`5/10`) means `5-hi/10` (standard cron).
			end = stepStr === undefined ? start : hi;
		}
		if (!Number.isInteger(start) || !Number.isInteger(end)) {
			throw new Error(`cron: non-numeric field "${part}"`);
		}
		for (let v = start; v <= end; v += step) {
			const n = isDow && v === 7 ? 0 : v;
			if (n < lo || n > hi) {
				throw new Error(
					`cron: value ${v} out of range [${lo}-${hi}] in "${part}"`,
				);
			}
			out.add(n);
		}
	}
	if (out.size === 0) throw new Error(`cron: empty field "${raw}"`);
	return out;
}

export function parseCron(expr: string): CronSpec {
	const fields = expr.trim().split(/\s+/);
	if (fields.length !== 5) {
		throw new Error(
			`cron: expected 5 fields, got ${fields.length} in "${expr}"`,
		);
	}
	for (const f of fields) {
		if (/[a-zA-Z]/.test(f)) {
			throw new Error(`cron: month/weekday names are not supported ("${f}")`);
		}
	}
	const [min, hr, dom, mon, dow] = fields as [
		string,
		string,
		string,
		string,
		string,
	];
	return {
		minute: parseField(min, 0, 59, false),
		hour: parseField(hr, 0, 23, false),
		dom: parseField(dom, 1, 31, false),
		month: parseField(mon, 1, 12, false),
		dow: parseField(dow, 0, 6, true),
		domRestricted: dom !== "*",
		dowRestricted: dow !== "*",
	};
}

/**
 * True iff the local minute represented by `date` satisfies every field.
 * Seconds/millis are ignored. dom/dow use OR-semantics when BOTH are restricted
 * (standard cron): a date matches if it satisfies either. When only one is
 * restricted, only that one constrains; when neither, the day always matches.
 */
export function cronMatches(spec: CronSpec, date: Date): boolean {
	if (!spec.minute.has(date.getMinutes())) return false;
	if (!spec.hour.has(date.getHours())) return false;
	if (!spec.month.has(date.getMonth() + 1)) return false;
	const domOk = spec.dom.has(date.getDate());
	const dowOk = spec.dow.has(date.getDay());
	if (spec.domRestricted && spec.dowRestricted) return domOk || dowOk;
	if (spec.domRestricted) return domOk;
	if (spec.dowRestricted) return dowOk;
	return true;
}

/**
 * Longest span `cronDue` scans backward, in minutes (48h). Bounds per-tick work
 * if a cursor is somehow far in the past; a match anywhere in the clamped window
 * still fires exactly once (catch-up-once).
 */
export const CRON_LOOKBACK_MINUTES = 48 * 60;

/**
 * True iff at least one whole minute `m` with `lastCheckedMs < m <= nowMs`
 * satisfies `spec` (local time). Walks minute boundaries from the clamped lower
 * bound to now and returns on the first match — so the caller fires ONCE even
 * when many matching slots were missed.
 */
export function cronDue(
	spec: CronSpec,
	lastCheckedMs: number,
	nowMs: number,
): boolean {
	if (nowMs <= lastCheckedMs) return false;
	const MIN = 60_000;
	// First whole-minute epoch strictly after lastChecked.
	let m = Math.floor(lastCheckedMs / MIN) * MIN + MIN;
	const floor = Math.floor(nowMs / MIN) * MIN - CRON_LOOKBACK_MINUTES * MIN;
	if (m < floor) m = floor;
	for (; m <= nowMs; m += MIN) {
		if (cronMatches(spec, new Date(m))) return true;
	}
	return false;
}
