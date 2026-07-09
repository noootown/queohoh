import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

export default defineConfig({
	// Resolve the workspace dependency to its TypeScript source so tests always
	// exercise fresh core code. The published entry point (package.json exports)
	// points at compiled dist/, which would otherwise require a build step and
	// risk running tests against a stale artifact.
	resolve: {
		alias: {
			"@queohoh/core": fileURLToPath(
				new URL("../core/src/index.ts", import.meta.url),
			),
		},
	},
	test: { include: ["src/**/*.test.ts"] },
});
