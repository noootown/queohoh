import {
	getUsageProbe,
	type ProviderUsage,
	type UsageProbe,
	type UsageSample,
	type UsageSeverity,
} from "@queohoh/core";

/**
 * Last successful sample per provider this process (design: provider-usage-header).
 * Not on the wire — only seeds stale publishes on fail.
 */
type CacheEntry = {
	text: string;
	severity: UsageSeverity;
	fetchedAt: number;
};

export interface UsagePollerDeps {
	/**
	 * Enabled provider names in display/poll order (settings precedence).
	 * Re-read each tick so config/settings changes take effect without restart.
	 */
	providers: () => string[];
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
 * Polls usage for EVERY enabled provider on interval + immediate on start.
 * Owns per-provider cache, stale flags, in-flight coalesce, and late-response
 * discard so the API can merge `snapshot()` into StateSnapshot without knowing
 * about probes.
 *
 * Failure never throws out of the poller (null/stale path only). Providers with
 * no probe or a permanently-null probe simply omit that name from snapshot().
 */
export class UsagePoller {
	private readonly providers: () => string[];
	private readonly getProbe: (provider: string) => UsageProbe | null;
	private readonly onChange: () => void;
	private readonly now: () => number;
	private readonly intervalMs: number;

	private cache = new Map<string, CacheEntry>();
	/** Last published sample per provider (success or last-good stale). */
	private published = new Map<string, ProviderUsage>();
	/**
	 * Monotonic generation per provider. Captured as `myGen` per flight so a
	 * late response from a superseded fetch cannot overwrite a newer one.
	 */
	private epoch = new Map<string, number>();
	/** Provider → generation that currently owns the in-flight slot. */
	private inFlight = new Map<string, number>();
	/** After stop(), ignore late completions and further refresh work. */
	private stopped = true;
	private timer: ReturnType<typeof setInterval> | null = null;

	constructor(deps: UsagePollerDeps) {
		this.providers = deps.providers;
		this.getProbe = deps.getProbe ?? getUsageProbe;
		this.onChange = deps.onChange;
		this.now = deps.now ?? (() => Date.now());
		this.intervalMs = deps.intervalMs ?? 60_000;
	}

	/** Start interval + immediate refresh for all enabled providers. */
	start(): void {
		this.stopped = false;
		if (this.timer !== null) {
			clearInterval(this.timer);
		}
		this.timer = setInterval(() => {
			void this.refreshAll();
		}, this.intervalMs);
		void this.refreshAll();
	}

	stop(): void {
		this.stopped = true;
		if (this.timer !== null) {
			clearInterval(this.timer);
			this.timer = null;
		}
	}

	/**
	 * Samples currently on the wire, in `providers()` order. Providers with no
	 * successful sample yet (or a null-only probe) are omitted.
	 */
	snapshot(): ProviderUsage[] {
		const out: ProviderUsage[] = [];
		for (const name of this.providers()) {
			const u = this.published.get(name);
			if (u) out.push(u);
		}
		return out;
	}

	private async refreshAll(): Promise<void> {
		if (this.stopped) return;

		const names = this.providers();
		// Drop published entries for providers no longer enabled/listed so the
		// wire doesn't keep showing a disabled provider's last sample.
		let pruned = false;
		for (const key of [...this.published.keys()]) {
			if (!names.includes(key)) {
				this.published.delete(key);
				pruned = true;
			}
		}
		if (pruned) this.onChange();

		await Promise.all(names.map((p) => this.refreshOne(p)));
	}

	private async refreshOne(P: string): Promise<void> {
		if (this.stopped) return;

		const probe = this.getProbe(P);
		if (probe === null) {
			if (this.published.has(P)) {
				this.published.delete(P);
				this.onChange();
			}
			return;
		}

		// Coalesce: at most one in-flight fetch per provider name.
		if (this.inFlight.has(P)) {
			return;
		}

		const myGen = (this.epoch.get(P) ?? 0) + 1;
		this.epoch.set(P, myGen);
		this.inFlight.set(P, myGen);

		let sample: UsageSample | null;
		try {
			sample = await probe.fetch();
		} catch {
			// Failures never throw into callers; treat like a null sample.
			sample = null;
		}

		// Clear coalesce slot only if this flight still owns it.
		if (this.inFlight.get(P) === myGen) {
			this.inFlight.delete(P);
		}

		// Discard after stop, or a newer flight for the same provider.
		if (this.stopped) return;
		if (this.epoch.get(P) !== myGen) return;
		// Provider dropped from the enabled list while we were in flight.
		if (!this.providers().includes(P)) return;

		if (sample !== null) {
			const fetchedAt = this.now();
			this.cache.set(P, {
				text: sample.text,
				severity: sample.severity,
				fetchedAt,
			});
			this.published.set(P, {
				provider: P,
				text: sample.text,
				severity: sample.severity,
				fetchedAt,
				stale: false,
			});
			this.onChange();
			return;
		}

		// Fetch failed / null: last-good stale, or drop the chip for this provider.
		const cached = this.cache.get(P);
		if (cached) {
			this.published.set(P, {
				provider: P,
				text: cached.text,
				severity: cached.severity,
				fetchedAt: cached.fetchedAt,
				stale: true,
			});
		} else {
			this.published.delete(P);
		}
		this.onChange();
	}
}
