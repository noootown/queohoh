import { readFile } from "node:fs/promises";
import { homedir } from "node:os";
import { join } from "node:path";
import {
	maxSeverity,
	type UsageFetch,
	type UsageProbe,
	type UsageSample,
} from "./usage.js";

export type { UsageFetch } from "./usage.js";

const DEFAULT_BASE_URL = "https://cli-chat-proxy.grok.com/v1";
const DEFAULT_TIMEOUT_MS = 5000;

export type GrokTokenReader = () => Promise<string | null>;

/**
 * Pure parse of Grok billing JSON (monthly + optional weekly credits) →
 * sample or null. Divide-by-zero / missing monthly fields → null.
 * Weekly is optional: null/unusable weekly → monthly-only text.
 */
export function parseGrokBilling(
	monthlyJson: unknown,
	weeklyJson: unknown | null,
): UsageSample | null {
	const monthlyPct = readMonthlyPercent(monthlyJson);
	if (monthlyPct === null) return null;

	const weeklyPct = weeklyJson == null ? null : readWeeklyPercent(weeklyJson);
	if (weeklyPct === null) {
		return {
			text: `${Math.round(monthlyPct)}% mo`,
			severity: maxSeverity([monthlyPct]),
		};
	}
	return {
		text: `${Math.round(monthlyPct)}%/${Math.round(weeklyPct)}%`,
		severity: maxSeverity([monthlyPct, weeklyPct]),
	};
}

function readMonthlyPercent(json: unknown): number | null {
	if (json === null || typeof json !== "object") return null;
	const config = (json as Record<string, unknown>).config;
	if (config === null || typeof config !== "object") return null;
	const cfg = config as Record<string, unknown>;
	const limit = readValNumber(cfg.monthlyLimit);
	const used = readValNumber(cfg.used);
	if (limit === null || used === null) return null;
	if (limit === 0) return null;
	const pct = (used / limit) * 100;
	if (!Number.isFinite(pct)) return null;
	return pct;
}

function readValNumber(field: unknown): number | null {
	if (field === null || typeof field !== "object") return null;
	const val = (field as Record<string, unknown>).val;
	if (typeof val !== "number" || !Number.isFinite(val)) return null;
	return val;
}

/**
 * Weekly credits payload. Requires USAGE_PERIOD_TYPE_WEEKLY; missing
 * creditUsagePercent → 0 (fresh period). Wrong shape → null (caller keeps monthly-only).
 */
function readWeeklyPercent(json: unknown): number | null {
	if (json === null || typeof json !== "object") return null;
	const config = (json as Record<string, unknown>).config;
	if (config === null || typeof config !== "object") return null;
	const cfg = config as Record<string, unknown>;
	const period = cfg.currentPeriod;
	if (period === null || typeof period !== "object") return null;
	if ((period as Record<string, unknown>).type !== "USAGE_PERIOD_TYPE_WEEKLY") {
		return null;
	}
	const raw = cfg.creditUsagePercent;
	if (raw === undefined || raw === null) return 0;
	if (typeof raw !== "number" || !Number.isFinite(raw)) return null;
	return raw;
}

/**
 * Default: `~/.grok/auth.json` first object value's `key` string.
 * Missing/invalid → null. Never log the secret.
 */
export async function readGrokTokenFromAuthFile(): Promise<string | null> {
	try {
		const path = join(homedir(), ".grok", "auth.json");
		const raw = await readFile(path, "utf8");
		const parsed: unknown = JSON.parse(raw);
		if (
			parsed === null ||
			typeof parsed !== "object" ||
			Array.isArray(parsed)
		) {
			return null;
		}
		const values = Object.values(parsed as Record<string, unknown>);
		if (values.length === 0) return null;
		const first = values[0];
		if (first === null || typeof first !== "object") return null;
		const key = (first as Record<string, unknown>).key;
		if (typeof key !== "string" || key.length === 0) return null;
		return key;
	} catch {
		return null;
	}
}

const defaultFetch: UsageFetch = async (url, init) => {
	const res = await fetch(url, {
		headers: init.headers,
		signal: init.signal,
	});
	return {
		ok: res.ok,
		status: res.status,
		json: () => res.json() as Promise<unknown>,
	};
};

/**
 * Grok billing usage probe. Inject `readToken` / `fetchImpl` in tests;
 * production defaults to auth.json + global fetch. Monthly required; weekly
 * soft-fails to monthly-only. `fetch()` never throws.
 */
export function createGrokUsageProbe(opts?: {
	readToken?: GrokTokenReader;
	fetchImpl?: UsageFetch;
	baseUrl?: string;
	timeoutMs?: number;
}): UsageProbe {
	const readToken = opts?.readToken ?? readGrokTokenFromAuthFile;
	const fetchImpl = opts?.fetchImpl ?? defaultFetch;
	const baseUrl = (opts?.baseUrl ?? DEFAULT_BASE_URL).replace(/\/$/, "");
	const timeoutMs = opts?.timeoutMs ?? DEFAULT_TIMEOUT_MS;

	return {
		provider: "grok",
		async fetch() {
			try {
				const token = await readToken();
				if (!token) return null;

				const controller = new AbortController();
				const timer = setTimeout(() => controller.abort(), timeoutMs);
				const headers = {
					Authorization: `Bearer ${token}`,
					"x-xai-token-auth": "xai-grok-cli",
					Accept: "application/json",
				};
				try {
					const monthlyRes = await fetchImpl(`${baseUrl}/billing`, {
						headers,
						signal: controller.signal,
					});
					if (!monthlyRes.ok) return null;
					const monthlyJson = await monthlyRes.json();

					let weeklyJson: unknown | null = null;
					try {
						const weeklyRes = await fetchImpl(
							`${baseUrl}/billing?format=credits`,
							{
								headers,
								signal: controller.signal,
							},
						);
						if (weeklyRes.ok) {
							weeklyJson = await weeklyRes.json();
						}
					} catch {
						// weekly optional — keep monthly-only
					}

					return parseGrokBilling(monthlyJson, weeklyJson);
				} finally {
					clearTimeout(timer);
				}
			} catch {
				return null;
			}
		},
	};
}
