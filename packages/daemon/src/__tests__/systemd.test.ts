import { describe, expect, it } from "vitest";
import { escapeSystemdValue, systemdUnit } from "../systemd.js";

describe("escapeSystemdValue", () => {
	it("leaves simple absolute paths unquoted", () => {
		expect(escapeSystemdValue("/usr/bin/node")).toBe("/usr/bin/node");
		expect(
			escapeSystemdValue("/home/me/.local/state/queohoh/daemon/daemon.log"),
		).toBe("/home/me/.local/state/queohoh/daemon/daemon.log");
	});

	it("quotes values with whitespace and escapes inner quotes/backslashes", () => {
		expect(escapeSystemdValue("/opt/my node/bin/node")).toBe(
			'"/opt/my node/bin/node"',
		);
		expect(escapeSystemdValue('/tmp/a"b.log')).toBe('"/tmp/a\\"b.log"');
		expect(escapeSystemdValue("/tmp/a\\b.log")).toBe('"/tmp/a\\\\b.log"');
	});

	it("doubles percent signs so systemd does not expand specifiers", () => {
		expect(escapeSystemdValue("/tmp/%i.log")).toBe("/tmp/%%i.log");
		expect(escapeSystemdValue("/tmp/%i %j.log")).toBe('"/tmp/%%i %%j.log"');
	});
});

describe("systemdUnit", () => {
	it("renders a KeepAlive-equivalent user unit with KillMode=process", () => {
		const unit = systemdUnit({
			nodeBin: "/usr/local/bin/node",
			cliPath: "/opt/queohoh/cli.js",
			logPath: "/tmp/queohoh.log",
		});
		expect(unit).toContain("[Unit]");
		expect(unit).toContain("Description=queohoh task-queue daemon");
		expect(unit).toContain("[Service]");
		expect(unit).toContain("Type=simple");
		expect(unit).toContain(
			"ExecStart=/usr/local/bin/node /opt/queohoh/cli.js daemon",
		);
		expect(unit).toContain("Restart=always");
		expect(unit).toContain("RestartSec=2");
		// Detached run shims must survive daemon restarts — default control-group
		// kill would tear them down with the service cgroup.
		expect(unit).toContain("KillMode=process");
		expect(unit).toContain("StandardOutput=append:/tmp/queohoh.log");
		expect(unit).toContain("StandardError=append:/tmp/queohoh.log");
		expect(unit).toContain("[Install]");
		expect(unit).toContain("WantedBy=default.target");
	});

	it("escapes interpolated ExecStart / log paths", () => {
		const unit = systemdUnit({
			nodeBin: "/opt/my node/bin/node",
			cliPath: "/opt/queohoh/cli.js",
			logPath: "/tmp/%i.log",
		});
		expect(unit).toContain(
			'ExecStart="/opt/my node/bin/node" /opt/queohoh/cli.js daemon',
		);
		expect(unit).toContain("StandardOutput=append:/tmp/%%i.log");
		expect(unit).not.toContain("append:/tmp/%i.log");
	});
});
