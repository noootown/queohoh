You are in the primary checkout of the {{project}} repo at {{repo_path}}. Squash-merge the branch `{{source}}` into `{{target}}` as a single commit, then remove the source worktree and branch.

## Preconditions — verify all before touching anything

Abort immediately with a clear one-line reason if any of these fail:

1. `{{source}}` and `{{target}}` are different branches.
2. The working tree here is clean (`git status --porcelain` is empty). If not, abort — never stash or discard the user's work.
3. The branch `{{source}}` exists (`git rev-parse --verify {{source}}`).
4. `git log --oneline {{target}}..{{source}}` is non-empty — if there is nothing to squash, abort and say so.

## Merge

1. Show the commits that will be squashed: `git log --oneline {{target}}..{{source}}`.
2. Check out the target: `git checkout {{target}}`. If this fails because `{{target}}` is checked out in another worktree, abort with that error verbatim.
3. Squash: `git merge --squash {{source}}`. On conflicts, resolve them yourself:
   - For each conflicted file, understand what each side was doing (`git log --oneline {{target}}...{{source}} -- <file>` and the surrounding code) and write a resolution that preserves the intent of BOTH branches — never pick one side wholesale unless it is a strict superset of the other.
   - Stage each resolved file with `git add`, and make sure no conflict markers survive (`git diff --check`).
   - Verify the resolved tree before committing: run the project's build/tests if any are configured (mise tasks, package scripts, cargo, …). A resolution that doesn't build or pass is not a resolution — keep fixing it.
   - Only if a conflict is genuinely unresolvable — the two branches made incompatible semantic choices that need a human decision — restore a clean tree (`git merge --abort` if mid-merge, otherwise `git reset --hard`) and report which files conflicted and what decision is needed.
4. Write a conventional commit message from the staged diff: a `type(scope): subject` title that describes the net change (not the branch name), plus a short body when the diff spans multiple concerns. Commit with it.

## Cleanup

After the commit lands on `{{target}}`:

1. Remove the worktree: `wt remove {{source}} --yes` (ignore failure if no worktree exists for the branch).
2. Delete the branch if it survived: `git branch -D {{source}}` (ignore "not found").

## Report

End with a short summary: how many commits were squashed, the new commit hash and title on `{{target}}`, and confirmation that the `{{source}}` worktree and branch were removed.
