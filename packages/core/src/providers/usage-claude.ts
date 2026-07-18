import { execFile } from "node:child_process";
import { promisify } from "node:util";
import {
	maxSeverity,
	type UsageFetch,
	type UsageProbe,
	type UsageSample,
} from "./usage.js";

export type { UsageFetch } from "./usage.js";

const execFileAsync = promisify(execFile);

const OAUTH_USAGE_URL = "https://api.anthropic.com/api/oauth/usage";
const DEFAULT_TIMEOUT_MS = 5000;

export type ClaudeTokenReader = () => Promise<string | null>;

/** Pure parse of Anthropic OAuth usage JSON → sample or null. */
export function parseClaudeUsage(json: unknown): UsageSample | null {
	if (json === null || typeof json !== "object") return null;
	const obj = json as Record<string, unknown>;
	const session = readUtilization(obj.five_hour);
	const week = readUtilization(obj.seven_day);
	if (session === null || week === null) return null;
	return {
		text: `${Math.round(session)}%/${Math.round(week)}%`,
		severity: maxSeverity([session, week]),
	};
}

function readUtilization(bucket: unknown): number | null {
	if (bucket === null || typeof bucket !== "object") return null;
	const utilization = (bucket as Record<string, unknown>).utilization;
	if (typeof utilization !== "number" || !Number.isFinite(utilization)) {
		return null;
	}
	return utilization;
}

/**
 * Default: macOS Keychain service `Claude Code-credentials` →
 * `claudeAiOauth.accessToken`. Missing/fail → null. Never log the secret.
 */
export async function readClaudeOAuthTokenFromKeychain(): Promise<
	string | null
> {
	try {
		const { stdout } = await execFileAsync("security", [
			"find-generic-password",
			"-s",
			"Claude Code-credentials",
			"-w",
		]);
		const parsed: unknown = JSON.parse(stdout.trim());
		if (parsed === null || typeof parsed !== "object") return null;
		const oauth = (parsed as Record<string, unknown>).claudeAiOauth;
		if (oauth === null || typeof oauth !== "object") return null;
		const token = (oauth as Record<string, unknown>).accessToken;
		if (typeof token !== "string" || token.length === 0) return null;
		return token;
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
 * Claude OAuth usage probe. Inject `readToken` / `fetchImpl` in tests;
 * production defaults to keychain + global fetch. `fetch()` never throws.
 */
export function createClaudeUsageProbe(opts?: {
	readToken?: ClaudeTokenReader;
	fetchImpl?: UsageFetch;
	timeoutMs?: number;
}): UsageProbe {
	const readToken = opts?.readToken ?? readClaudeOAuthTokenFromKeychain;
	const fetchImpl = opts?.fetchImpl ?? defaultFetch;
	const timeoutMs = opts?.timeoutMs ?? DEFAULT_TIMEOUT_MS;

	return {
		provider: "claude",
		async fetch() {
			try {
				const token = await readToken();
				if (!token) return null;

				const controller = new AbortController();
				const timer = setTimeout(() => controller.abort(), timeoutMs);
				try {
					const res = await fetchImpl(OAUTH_USAGE_URL, {
						headers: {
							Authorization: `Bearer ${token}`,
							"anthropic-beta": "oauth-2025-04-20",
							Accept: "application/json",
						},
						signal: controller.signal,
					});
					if (!res.ok) return null;
					const json = await res.json();
					return parseClaudeUsage(json);
				} finally {
					clearTimeout(timer);
				}
			} catch {
				// missing token already handled; timeout/throw/parse null → null
				return null;
			}
		},
	};
}
