import { describe, expect, it } from "vitest";
import type { ResolverIO, WorktreeInfo } from "../resolver.js";
import { resolveTarget } from "../resolver.js";

function stubIO(overrides: Partial<ResolverIO> = {}): ResolverIO & {
	spawned: { name: string; branch?: string }[];
} {
	const spawned: { name: string; branch?: string }[] = [];
	return {
		spawned,
		listWorktrees: async () => [],
		prBranch: async () => null,
		spawnWorktree: async (_repo, name, branch) => {
			spawned.push({ name, branch });
			return { name, path: `/wt/${name}`, branch: branch ?? name };
		},
		removeWorktree: async () => {},
		...overrides,
	};
}

const wt = (name: string, branch = name): WorktreeInfo => ({
	name,
	path: `/wt/${name}`,
	branch,
});

const ctx = { repoPath: "/repo", tempName: () => "qoo-fix-abc123" };

describe("resolveTarget", () => {
	it("worktree ref: uses existing", async () => {
		const io = stubIO({ listWorktrees: async () => [wt("main")] });
		expect(await resolveTarget("worktree:main", ctx, io)).toEqual({
			outcome: "resolved",
			worktree: "main",
			ephemeral: false,
		});
	});

	it("worktree ref: needs-input when absent", async () => {
		const result = await resolveTarget("worktree:gone", ctx, stubIO());
		expect(result.outcome).toBe("needs-input");
	});

	it("pr ref: matches existing worktree by branch", async () => {
		const io = stubIO({
			prBranch: async () => "JUS-1423-fix-auth",
			listWorktrees: async () => [wt("anything", "JUS-1423-fix-auth")],
		});
		expect(await resolveTarget("pr:1423", ctx, io)).toEqual({
			outcome: "resolved",
			worktree: "anything",
			ephemeral: false,
		});
	});

	it("pr ref: spawns a branch-named worktree", async () => {
		const io = stubIO({ prBranch: async () => "JUS-1423-fix-auth" });
		expect(await resolveTarget("pr:1423", ctx, io)).toEqual({
			outcome: "resolved",
			worktree: "JUS-1423-fix-auth",
			ephemeral: false,
		});
		expect(io.spawned).toEqual([
			{ name: "JUS-1423-fix-auth", branch: "JUS-1423-fix-auth" },
		]);
	});

	it("pr ref: reuses an existing worktree named like the branch", async () => {
		const io = stubIO({
			prBranch: async () => "JUS-1423-fix-auth",
			listWorktrees: async () => [wt("JUS-1423-fix-auth", "other-branch")],
		});
		expect(await resolveTarget("pr:1423", ctx, io)).toEqual({
			outcome: "resolved",
			worktree: "JUS-1423-fix-auth",
			ephemeral: false,
		});
		expect(io.spawned).toEqual([]);
	});

	it("pr ref: folds branch slashes into the worktree name", async () => {
		const io = stubIO({
			prBranch: async () => "dependabot/npm_and_yarn/npm-0846159061",
		});
		expect(await resolveTarget("pr:1821", ctx, io)).toEqual({
			outcome: "resolved",
			worktree: "dependabot-npm_and_yarn-npm-0846159061",
			ephemeral: false,
		});
		expect(io.spawned).toEqual([
			{
				name: "dependabot-npm_and_yarn-npm-0846159061",
				branch: "dependabot/npm_and_yarn/npm-0846159061",
			},
		]);
	});

	it("pr ref: needs-input when pr not found", async () => {
		const result = await resolveTarget("pr:9999", ctx, stubIO());
		expect(result.outcome).toBe("needs-input");
	});

	it("ticket ref: uses existing worktree named by ticket", async () => {
		const io = stubIO({ listWorktrees: async () => [wt("JUS-77")] });
		expect(await resolveTarget("ticket:JUS-77", ctx, io)).toEqual({
			outcome: "resolved",
			worktree: "JUS-77",
			ephemeral: false,
		});
		expect(io.spawned).toEqual([]);
	});

	it("ticket ref: spawns when absent", async () => {
		const io = stubIO();
		expect(await resolveTarget("ticket:JUS-77", ctx, io)).toEqual({
			outcome: "resolved",
			worktree: "JUS-77",
			ephemeral: false,
		});
		expect(io.spawned).toEqual([{ name: "JUS-77", branch: undefined }]);
	});

	it("temp ref: spawns ephemeral with generated name", async () => {
		const io = stubIO();
		expect(await resolveTarget("temp", ctx, io)).toEqual({
			outcome: "resolved",
			worktree: "qoo-fix-abc123",
			ephemeral: true,
		});
	});

	it("temp ref: defaults to a qoo-prefixed name when no tempName is given", async () => {
		const io = stubIO();
		const result = await resolveTarget("temp", { repoPath: "/repo" }, io);
		expect(result).toMatchObject({ outcome: "resolved", ephemeral: true });
		expect(io.spawned[0]?.name).toMatch(/^qoo-[0-9a-z]{6}$/);
	});

	it("repo ref: resolves to the @repo sentinel, never spawns", async () => {
		const io = stubIO();
		expect(await resolveTarget("repo", ctx, io)).toEqual({
			outcome: "resolved",
			worktree: "@repo",
			ephemeral: false,
		});
		expect(io.spawned).toEqual([]);
	});

	it("garbage ref: needs-input, never throws", async () => {
		const result = await resolveTarget("wat:?", ctx, stubIO());
		expect(result.outcome).toBe("needs-input");
	});
});
