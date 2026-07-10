import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

export default defineConfig({
	esbuild: { jsx: "automatic" },
	resolve: {
		alias: {
			"@queohoh/daemon": fileURLToPath(
				new URL("../daemon/src/index.ts", import.meta.url),
			),
			"@queohoh/core": fileURLToPath(
				new URL("../core/src/index.ts", import.meta.url),
			),
		},
	},
	test: {
		include: ["src/**/*.test.ts", "src/**/*.test.tsx"],
		// Force chalk/ink to emit ANSI escapes deterministically in non-TTY runs
		// (CI, piped output) so render tests can assert on inverse-highlight (`[7m`)
		// output; without this the range-selection tests only see plain text.
		env: { FORCE_COLOR: "3" },
	},
});
