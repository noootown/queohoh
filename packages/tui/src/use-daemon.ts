import type { StateSnapshot } from "@queohoh/daemon";
import { ApiClient } from "@queohoh/daemon";
import { useEffect, useRef, useState } from "react";

export interface DaemonState {
	snapshot: StateSnapshot | null;
	connected: boolean;
}

/**
 * Coerce an untrusted snapshot from the socket into a well-formed
 * `StateSnapshot`, filling missing/wrong-typed fields with safe defaults.
 *
 * The wire value is `unknown`: a daemon running an OLDER build sends snapshots
 * that predate newer fields (`projects`, `worktrees`, `maxConcurrent`). Without
 * this the TUI trusts its compile-time type and crashes (e.g.
 * `snapshot.projects.map(...)` on `undefined`). This is the single choke point
 * for both ingestion paths, so callers never touch raw wire data.
 */
export function normalizeSnapshot(raw: unknown): StateSnapshot {
	const r = (typeof raw === "object" && raw !== null ? raw : {}) as Record<
		string,
		unknown
	>;
	const arr = <K extends keyof StateSnapshot>(key: K): StateSnapshot[K] =>
		(Array.isArray(r[key]) ? r[key] : []) as StateSnapshot[K];
	const worktrees =
		typeof r.worktrees === "object" &&
		r.worktrees !== null &&
		!Array.isArray(r.worktrees)
			? (r.worktrees as StateSnapshot["worktrees"])
			: {};
	const mainSessions =
		typeof r.mainSessions === "object" &&
		r.mainSessions !== null &&
		!Array.isArray(r.mainSessions)
			? (r.mainSessions as StateSnapshot["mainSessions"])
			: {};
	return {
		tasks: arr("tasks"),
		archivedRecent: arr("archivedRecent"),
		sessions: arr("sessions"),
		running: arr("running"),
		// App reads `snapshot?.maxConcurrent ?? null`. Keep a real number when the
		// daemon sends one, else stay nullish so the header omits "/M" exactly as
		// it did before this field existed (old daemons send no maxConcurrent).
		maxConcurrent: (typeof r.maxConcurrent === "number"
			? r.maxConcurrent
			: null) as number,
		projects: arr("projects"),
		worktrees,
		mainSessions,
		// Preserve buildId as-is; a pre-feature daemon omits it, and the self-heal
		// logic treats that `undefined` as stale on purpose. Do NOT default it.
		buildId: typeof r.buildId === "string" ? r.buildId : undefined,
	};
}

export function useDaemon(
	sockPath: string,
	opts?: { retryMs?: number },
): DaemonState {
	const retryMs = opts?.retryMs ?? 2000;
	const [state, setState] = useState<DaemonState>({
		snapshot: null,
		connected: false,
	});
	const alive = useRef(true);

	useEffect(() => {
		alive.current = true;
		let client: ApiClient | null = null;
		let retryTimer: NodeJS.Timeout | null = null;

		const attempt = async () => {
			if (!alive.current) return;
			client = new ApiClient();
			try {
				await client.connect(sockPath);
				client.onClose(() => {
					if (!alive.current) return;
					setState((prev) => ({ ...prev, connected: false }));
					scheduleRetry();
				});
				await client.subscribe((pushed) => {
					if (alive.current) {
						setState({ snapshot: normalizeSnapshot(pushed), connected: true });
					}
				});
				const initial = normalizeSnapshot(await client.call("state"));
				if (alive.current) setState({ snapshot: initial, connected: true });
			} catch {
				client.close();
				if (alive.current) {
					setState((prev) => ({ ...prev, connected: false }));
					scheduleRetry();
				}
			}
		};

		const scheduleRetry = () => {
			if (retryTimer) clearTimeout(retryTimer);
			retryTimer = setTimeout(() => void attempt(), retryMs);
		};

		void attempt();
		return () => {
			alive.current = false;
			if (retryTimer) clearTimeout(retryTimer);
			client?.close();
		};
	}, [sockPath, retryMs]);

	return state;
}
