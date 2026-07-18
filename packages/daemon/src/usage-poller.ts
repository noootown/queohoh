import {
	getUsageProbe,
	type ProviderUsage,
	type UsageProbe,
	type UsageSample,
	type UsageSeverity,
} from "@queohoh/core";

/**
 * Last successful sample per provider this process (design: provider-usage-header).
 * Not on the wire — only seeds stale publishes on fail/switch.
 */
type CacheEntry = {
	text: string;
	severity: UsageSeverity;
	fetchedAt: number;
};

export interface UsagePollerDeps {
	/** Current active provider name. */
	activeProvider: () => string;
	/** Resolve probe for a provider; default getUsageProbe. */
	getProbe?: (provider: string) => UsageProbe | null;
	/** Notify after published value may have changed (daemon wires → broadcast). */
	onChange: () => void;
	/** Clock for fetchedAt + tests. */
	now?: () => number;
	/** Poll interval; default 60_000. */
	intervalMs?: number;
}

/**
 * Polls usage for the active provider only: interval + immediate on start /
 * provider switch. Owns cache, stale flags, and late-response discard so the
 * API can merge `snapshot()` into StateSnapshot without knowing about probes.
 *
 * Failure never throws out of the poller (null/stale path only).
 */
export class UsagePoller {
	private readonly activeProvider: () => string;
	private readonly getProbe: (provider: string) => UsageProbe | null;
	private readonly onChange: () => void;
	private readonly now: () => number;
	private readonly intervalMs: number;

	private cache = new Map<string, CacheEntry>();
	private published: ProviderUsage | null = null;
	/** Provider name of the in-flight fetch, if any (coalesce same-P only). */
	private inFlightFor: string | null = null;
	/**
	 * Monotonic generation bumped when each fetch starts. Captured as `myGen`
	 * per flight so a late A1 after A→B→A cannot overwrite a newer A2 (same
	 * provider name is not enough — only the latest flight may publish).
	 */
	private epoch = 0;
	/** Generation that currently owns `inFlightFor` (clear only if matching). */
	private inFlightGen = 0;
	/** After stop(), ignore late completions and further refresh work. */
	private stopped = true;
	private timer: ReturnType<typeof setInterval> | null = null;

	constructor(deps: UsagePollerDeps) {
		this.activeProvider = deps.activeProvider;
		this.getProbe = deps.getProbe ?? getUsageProbe;
		this.onChange = deps.onChange;
		this.now = deps.now ?? (() => Date.now());
		this.intervalMs = deps.intervalMs ?? 60_000;
	}

	/** Start interval + immediate refresh for current active provider. */
	start(): void {
		this.stopped = false;
		if (this.timer !== null) {
			clearInterval(this.timer);
		}
		this.timer = setInterval(() => {
			void this.refresh(false);
		}, this.intervalMs);
		void this.refresh(false);
	}

	stop(): void {
		this.stopped = true;
		if (this.timer !== null) {
			clearInterval(this.timer);
			this.timer = null;
		}
	}

	/** Call after set_active_provider succeeds. Immediate refresh for new active. */
	onActiveProviderChanged(): void {
		void this.refresh(true);
	}

	/** Value to merge into StateSnapshot (null → omit / null on wire). */
	snapshot(): ProviderUsage | null {
		return this.published;
	}

	/**
	 * @param isSwitch — true on active-provider change: publish cache[P] as
	 *   stale (or null) before fetching so the UI flips immediately.
	 */
	private async refresh(isSwitch: boolean): Promise<void> {
		if (this.stopped) return;

		const P = this.activeProvider();

		if (isSwitch) {
			const cached = this.cache.get(P);
			if (cached) {
				this.published = {
					provider: P,
					text: cached.text,
					severity: cached.severity,
					fetchedAt: cached.fetchedAt,
					stale: true,
				};
			} else {
				this.published = null;
			}
			this.onChange();
		}

		const probe = this.getProbe(P);
		if (probe === null) {
			if (this.published !== null) {
				this.published = null;
				this.onChange();
			}
			return;
		}

		// Interval coalesce: at most one in-flight fetch per provider name.
		// (Does not apply across A→B→A: after switch away, inFlightFor is B,
		// so the return trip starts a new A flight with a new generation.)
		if (this.inFlightFor === P) {
			return;
		}

		const myGen = ++this.epoch;
		this.inFlightFor = P;
		this.inFlightGen = myGen;
		let sample: UsageSample | null;
		try {
			sample = await probe.fetch();
		} catch {
			// Failures never throw into callers; treat like a null sample.
			sample = null;
		}

		// Clear coalesce slot only if this flight still owns it (not a
		// superseded same-P flight that started after A→B→A).
		if (this.inFlightFor === P && this.inFlightGen === myGen) {
			this.inFlightFor = null;
		}

		// Discard after stop, active switch away, or a newer flight for any
		// provider (covers late A1 after A→B→A where active is again A).
		if (this.stopped) return;
		if (this.activeProvider() !== P) return;
		if (myGen !== this.epoch) return;

		if (sample !== null) {
			const fetchedAt = this.now();
			this.cache.set(P, {
				text: sample.text,
				severity: sample.severity,
				fetchedAt,
			});
			this.published = {
				provider: P,
				text: sample.text,
				severity: sample.severity,
				fetchedAt,
				stale: false,
			};
			this.onChange();
			return;
		}

		// Fetch failed / null: last-good stale, or hide chip.
		const cached = this.cache.get(P);
		if (cached) {
			this.published = {
				provider: P,
				text: cached.text,
				severity: cached.severity,
				fetchedAt: cached.fetchedAt,
				stale: true,
			};
		} else {
			this.published = null;
		}
		this.onChange();
	}
}
