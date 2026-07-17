#!/usr/bin/env node
import { existsSync, mkdirSync, unlinkSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { Command } from "commander";
import { ApiClient } from "./client.js";
import { startDaemon } from "./daemon.js";
import { launchdPlist } from "./launchd.js";
import { runMcpStdio } from "./mcp.js";
import { socketPath, statePath } from "./paths.js";
import { defaultReloadSteps, runReload } from "./reload.js";
import { systemdUnit } from "./systemd.js";

const PLIST_PATH = join(
	homedir(),
	"Library/LaunchAgents/com.queohoh.daemon.plist",
);

/** systemd user unit name (and basename under ~/.config/systemd/user/). */
const SYSTEMD_UNIT = "queohoh.daemon.service";
const SYSTEMD_UNIT_PATH = join(homedir(), ".config/systemd/user", SYSTEMD_UNIT);

const program = new Command();
program.name("queohoh").description("queohoh orchestrator daemon");

program
	.command("daemon")
	.description("run the daemon in the foreground")
	.action(async () => {
		await startDaemon();
	});

program
	.command("status")
	.description("print daemon state")
	.action(async () => {
		const client = new ApiClient();
		try {
			await client.connect(socketPath(statePath()));
			const state = await client.call("state");
			console.log(JSON.stringify(state, null, 2));
		} catch {
			console.error("daemon not reachable");
			process.exitCode = 1;
		} finally {
			client.close();
		}
	});

program
	.command("reload")
	.description(
		"rebuild this checkout and restart the daemon on the fresh build",
	)
	.option(
		"--force",
		"(no-op, kept for compatibility) reload always proceeds now",
		false,
	)
	.action(async () => {
		const cliPath = fileURLToPath(import.meta.url);
		process.exitCode = await runReload(defaultReloadSteps(cliPath), {
			info: console.log,
			error: console.error,
		});
	});

program
	.command("launchd:install")
	.description("write the launchd KeepAlive plist")
	.action(() => {
		mkdirSync(join(homedir(), "Library/LaunchAgents"), { recursive: true });
		mkdirSync(join(statePath(), "daemon"), { recursive: true });
		const cliPath = fileURLToPath(import.meta.url);
		// Snapshot the discovery env so launchd (no path.zsh) still finds the
		// config workspace the same way an interactive shell does.
		const env: Record<string, string> = {};
		for (const k of [
			"QUEOHOH_WORKSPACE",
			"QUEOHOH_CONFIG",
			"QUEOHOH_STATE_DIR",
		] as const) {
			const v = process.env[k];
			if (v && v.trim() !== "") env[k] = v;
		}
		writeFileSync(
			PLIST_PATH,
			launchdPlist({
				label: "com.queohoh.daemon",
				nodeBin: process.execPath,
				cliPath,
				logPath: join(statePath(), "daemon/daemon.log"),
				env,
			}),
		);
		console.log(`wrote ${PLIST_PATH}`);
		console.log(`activate: launchctl bootstrap gui/$(id -u) ${PLIST_PATH}`);
	});

program
	.command("launchd:uninstall")
	.description("remove the launchd plist")
	.action(() => {
		if (existsSync(PLIST_PATH)) unlinkSync(PLIST_PATH);
		console.log(
			`removed. deactivate: launchctl bootout gui/$(id -u)/com.queohoh.daemon`,
		);
	});

program
	.command("systemd:install")
	.description("write the systemd user unit (Linux keep-alive)")
	.action(() => {
		mkdirSync(join(homedir(), ".config/systemd/user"), { recursive: true });
		mkdirSync(join(statePath(), "daemon"), { recursive: true });
		const cliPath = fileURLToPath(import.meta.url);
		writeFileSync(
			SYSTEMD_UNIT_PATH,
			systemdUnit({
				nodeBin: process.execPath,
				cliPath,
				logPath: join(statePath(), "daemon/daemon.log"),
			}),
		);
		console.log(`wrote ${SYSTEMD_UNIT_PATH}`);
		console.log(
			`activate: systemctl --user daemon-reload && systemctl --user enable --now ${SYSTEMD_UNIT}`,
		);
	});

program
	.command("systemd:uninstall")
	.description("remove the systemd user unit")
	.action(() => {
		if (existsSync(SYSTEMD_UNIT_PATH)) unlinkSync(SYSTEMD_UNIT_PATH);
		console.log(
			`removed. deactivate: systemctl --user disable --now ${SYSTEMD_UNIT} && systemctl --user daemon-reload`,
		);
	});

program
	.command("mcp")
	.description("run the MCP stdio server (register in Claude Code)")
	.action(async () => {
		await runMcpStdio();
	});

program
	.command("heartbeat")
	.description("register an interactive session heartbeat (best-effort)")
	.option("--cwd <dir>", "session working directory", process.cwd())
	.action(async (opts: { cwd: string }) => {
		const client = new ApiClient();
		try {
			await client.connect(socketPath(statePath()));
			await client.call("heartbeatInteractive", {
				cwd: opts.cwd,
				pid: process.ppid,
			});
		} catch {
			// best-effort: never break a shell hook
		} finally {
			client.close();
		}
	});

program.parseAsync();
