/** Severity bucket for a provider usage sample. */
export type UsageSeverity = "ok" | "warn" | "crit" | "unknown";

export interface UsageSample {
	text: string;
	severity: UsageSeverity;
}

export interface ProviderUsage {
	provider: string;
	text: string;
	severity: UsageSeverity;
	fetchedAt: number;
	stale: boolean;
}

export interface UsageProbe {
	provider: string;
	fetch(): Promise<UsageSample | null>;
}

/** Injectable fetch for usage HTTP probes (Claude OAuth, Grok billing, …). */
export type UsageFetch = (
	url: string,
	init: { headers: Record<string, string>; signal?: AbortSignal },
) => Promise<{ ok: boolean; status: number; json(): Promise<unknown> }>;

const SEVERITY_RANK: Record<UsageSeverity, number> = {
	unknown: 0,
	ok: 1,
	warn: 2,
	crit: 3,
};

/** Map a percent-used value to severity. NaN/non-finite → unknown. */
export function severityFromPercent(pct: number): UsageSeverity {
	if (!Number.isFinite(pct)) return "unknown";
	if (pct < 70) return "ok";
	if (pct < 90) return "warn";
	return "crit";
}

/** severity of the worst (highest) bucket among finite percents; empty → unknown. */
export function maxSeverity(pcts: number[]): UsageSeverity {
	if (pcts.length === 0) return "unknown";
	let worst: UsageSeverity = "unknown";
	for (const pct of pcts) {
		const s = severityFromPercent(pct);
		if (SEVERITY_RANK[s] > SEVERITY_RANK[worst]) worst = s;
	}
	return worst;
}
