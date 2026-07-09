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
	// Serialized form of the last snapshot we committed, plus a mirror of the
	// connected flag. The daemon re-broadcasts a full, content-identical snapshot
	// on a fixed cadence; without a dedup every push would setState and re-render
	// the whole App for no change. These are refs (not `state`) so the skip
	// decision is made SYNCHRONOUSLY at push time — deciding inside the deferred
	// setState updater would race the ref against itself across batched pushes.
	const lastPushedJson = useRef<string | null>(null);
	const connectedRef = useRef(false);

	useEffect(() => {
		alive.current = true;
		let client: ApiClient | null = null;
		let retryTimer: NodeJS.Timeout | null = null;

		// Single ingress for the initial `state` reply and every pushed update.
		const applySnapshot = (pushed: unknown) => {
			if (!alive.current) return;
			const json = JSON.stringify(pushed);
			// Skip only when byte-identical AND already connected — a (re)connect
			// (connectedRef false) always commits so the fresh daemon's state lands
			// even if it matches the last snapshot seen before the disconnect.
			if (connectedRef.current && lastPushedJson.current === json) return;
			lastPushedJson.current = json;
			connectedRef.current = true;
			setState({ snapshot: normalizeSnapshot(pushed), connected: true });
		};
		const markDisconnected = () => {
			connectedRef.current = false;
			setState((prev) => ({ ...prev, connected: false }));
		};

		const attempt = async () => {
			if (!alive.current) return;
			client = new ApiClient();
			try {
				await client.connect(sockPath);
				client.onClose(() => {
					if (!alive.current) return;
					markDisconnected();
					scheduleRetry();
				});
				await client.subscribe((pushed) => applySnapshot(pushed));
				applySnapshot(await client.call("state"));
			} catch {
				client.close();
				if (alive.current) {
					markDisconnected();
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
