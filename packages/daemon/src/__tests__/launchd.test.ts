import { describe, expect, it } from "vitest";
import { launchdPlist } from "../launchd.js";

describe("launchdPlist", () => {
	it("renders KeepAlive plist with program arguments", () => {
		const xml = launchdPlist({
			label: "com.queohoh.daemon",
			nodeBin: "/usr/local/bin/node",
			cliPath: "/opt/queohoh/cli.js",
			logPath: "/tmp/queohoh.log",
		});
		expect(xml).toContain("<key>Label</key>");
		expect(xml).toContain("<string>com.queohoh.daemon</string>");
		expect(xml).toContain("<key>KeepAlive</key>");
		expect(xml).toContain("<true/>");
		expect(xml).toContain("<string>/usr/local/bin/node</string>");
		expect(xml).toContain("<string>/opt/queohoh/cli.js</string>");
		expect(xml).toContain("<string>daemon</string>");
		expect(xml).toContain("<string>/tmp/queohoh.log</string>");
	});

	it("XML-escapes interpolated strings", () => {
		const xml = launchdPlist({
			label: "a&b",
			nodeBin: "/usr/local/bin/node",
			cliPath: "/opt/<x>/cli.js",
			logPath: "/tmp/a>b.log",
		});
		expect(xml).toContain("<string>a&amp;b</string>");
		expect(xml).toContain("<string>/opt/&lt;x&gt;/cli.js</string>");
		expect(xml).toContain("<string>/tmp/a&gt;b.log</string>");
		expect(xml).not.toContain("<string>a&b</string>");
	});

	it("embeds non-empty EnvironmentVariables for workspace discovery", () => {
		const xml = launchdPlist({
			label: "com.queohoh.daemon",
			nodeBin: "/usr/local/bin/node",
			cliPath: "/opt/queohoh/cli.js",
			logPath: "/tmp/queohoh.log",
			env: {
				QUEOHOH_WORKSPACE: "/home/me/ws",
				QUEOHOH_CONFIG: "",
			},
		});
		expect(xml).toContain("<key>EnvironmentVariables</key>");
		expect(xml).toContain("<key>QUEOHOH_WORKSPACE</key>");
		expect(xml).toContain("<string>/home/me/ws</string>");
		expect(xml).not.toContain("QUEOHOH_CONFIG");
	});
});
