import type { SessionMode, TaskDefinition } from "@queohoh/core";
import { ApiClient } from "@queohoh/daemon";

export interface DefinitionSummary {
	repo: string;
	name: string;
	args: string[];
	hasDiscovery: boolean;
}

export interface EnqueueOptions {
	worktree?: string;
	session?: SessionMode;
}

export interface Actions {
	enqueue(
		prompt: string,
		repo: string,
		opts?: EnqueueOptions,
	): Promise<string | null>;
	retry(id: string): Promise<string | null>;
	skip(id: string): Promise<string | null>;
	setWorktree(id: string, worktree: string): Promise<string | null>;
	removeWorktree(repo: string, name: string): Promise<string | null>;
	runDefinition(
		repo: string,
		name: string,
		args: string[],
		worktree?: string,
	): Promise<string | null>;
	definition(repo: string, name: string): Promise<TaskDefinition | null>;
	definitions(): Promise<DefinitionSummary[]>;
}

async function withClient<T>(
	sockPath: string,
	fn: (client: ApiClient) => Promise<T>,
): Promise<T> {
	const client = new ApiClient();
	try {
		await client.connect(sockPath);
		return await fn(client);
	} finally {
		client.close();
	}
}

function asError(err: unknown): string {
	return err instanceof Error ? err.message : String(err);
}

export function createActions(sockPath: string): Actions {
	const mutate = async (
		method: string,
		params: Record<string, unknown>,
	): Promise<string | null> => {
		try {
			await withClient(sockPath, (c) => c.call(method, params));
			return null;
		} catch (err) {
			return asError(err);
		}
	};

	return {
		enqueue: (prompt, repo, opts) =>
			mutate("enqueue", {
				prompt,
				repo,
				...(opts?.worktree ? { worktree: opts.worktree } : {}),
				...(opts?.session ? { session: opts.session } : {}),
			}),
		retry: (id) => mutate("retry", { id }),
		skip: (id) => mutate("skip", { id }),
		setWorktree: (id, worktree) => mutate("setWorktree", { id, worktree }),
		removeWorktree: (repo, name) => mutate("removeWorktree", { repo, name }),
		runDefinition: async (repo, name, args, worktree) => {
			const result = await mutate("runDefinition", {
				repo,
				name,
				args,
				source: "tui",
				...(worktree ? { worktree } : {}),
			});
			// discovery can exceed the client timeout — the tasks may still land;
			// the push subscription re-syncs, so treat timeout as success.
			if (result?.includes("timed out")) return null;
			return result;
		},
		definition: async (repo, name) => {
			try {
				return await withClient(
					sockPath,
					(c) =>
						c.call("definition", { repo, name }) as Promise<TaskDefinition>,
				);
			} catch {
				return null;
			}
		},
		definitions: async () => {
			try {
				return await withClient(
					sockPath,
					(c) => c.call("definitions") as Promise<DefinitionSummary[]>,
				);
			} catch {
				return [];
			}
		},
	};
}
