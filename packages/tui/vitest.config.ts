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
	test: { include: ["src/**/*.test.ts", "src/**/*.test.tsx"] },
});
