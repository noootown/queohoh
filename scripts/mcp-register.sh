#!/usr/bin/env bash
# Register the queohoh MCP server with one or more agent CLIs (stdio).
#
# Usage:
#   scripts/mcp-register.sh              # every supported CLI present on PATH
#   scripts/mcp-register.sh claude       # only Claude Code
#   scripts/mcp-register.sh codex grok   # Codex + Grok Build
#
# Supported agents: claude, codex, grok. Missing CLIs are skipped with a
# warning; exit 1 only when none of the requested agents could be registered
# (or when node / the daemon CLI is missing).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CLI="${ROOT}/packages/daemon/dist/cli.js"
NAME="queohoh"

if ! command -v node >/dev/null 2>&1; then
	echo "node not found on PATH" >&2
	exit 1
fi
NODE="$(command -v node)"

if [ ! -f "$CLI" ]; then
	echo "daemon CLI not built: $CLI" >&2
	echo "run: mise run build   (or pnpm -r build)" >&2
	exit 1
fi

# Default: all supported agents. Positional args filter to a subset.
if [ "$#" -eq 0 ]; then
	AGENTS=(claude codex grok)
else
	AGENTS=("$@")
fi

ok=0
skipped=0
failed=0

register_claude() {
	# Claude's `mcp add` is not an upsert: drop a prior entry so re-runs are
	# idempotent (scope must match the install below).
	claude mcp remove --scope user "$NAME" >/dev/null 2>&1 || true
	claude mcp add --scope user "$NAME" -- "$NODE" "$CLI" mcp
}

register_codex() {
	# Codex also replaces by name only if we remove first; list/get fail on a
	# broken user config, so remove is best-effort.
	codex mcp remove "$NAME" >/dev/null 2>&1 || true
	codex mcp add "$NAME" -- "$NODE" "$CLI" mcp
}

register_grok() {
	# Grok's `mcp add` is documented as "add or update" — still remove first so
	# a stale command path is never left behind if update semantics change.
	grok mcp remove --scope user "$NAME" >/dev/null 2>&1 || true
	grok mcp add --scope user "$NAME" -- "$NODE" "$CLI" mcp
}

for agent in "${AGENTS[@]}"; do
	case "$agent" in
	claude | codex | grok) ;;
	*)
		echo "unknown agent: $agent (want claude|codex|grok)" >&2
		failed=$((failed + 1))
		continue
		;;
	esac

	if ! command -v "$agent" >/dev/null 2>&1; then
		echo "skip $agent — not on PATH"
		skipped=$((skipped + 1))
		continue
	fi

	echo "registering $NAME MCP with $agent …"
	if "register_${agent}"; then
		echo "  ok: $agent"
		ok=$((ok + 1))
	else
		echo "  failed: $agent" >&2
		failed=$((failed + 1))
	fi
done

if [ "$ok" -eq 0 ]; then
	echo "registered with 0 agents (skipped=$skipped failed=$failed)" >&2
	exit 1
fi

echo "registered with $ok agent(s) (skipped=$skipped failed=$failed)"
# Partial success (some agents failed) is still exit 0 — the user got at least
# one working registration; the failed line above is the signal.
exit 0
