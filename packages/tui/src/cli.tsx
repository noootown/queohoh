#!/usr/bin/env node
import { runsPath, socketPath, statePath } from "@queohoh/daemon";
import { render } from "ink";
import { App } from "./App.js";
import { createActions } from "./actions.js";
import { createAltScreen } from "./alt-screen.js";

const sock = socketPath(statePath());
const alt = createAltScreen();
alt.installGuards();
alt.enter();
const instance = render(
	<App
		sockPath={sock}
		runsDir={runsPath(statePath())}
		actions={createActions(sock)}
	/>,
);
void instance.waitUntilExit().then(() => alt.leave());
