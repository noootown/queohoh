export * from "./types.js";
export { getAdapter, registerAdapter } from "./registry.js";

import { claudeAdapter } from "./claude.js";
import { codexAdapter } from "./codex.js";
import { grokAdapter } from "./grok.js";
import { registerAdapter } from "./registry.js";

export { claudeAdapter } from "./claude.js";
export { codexAdapter } from "./codex.js";
export { grokAdapter } from "./grok.js";

registerAdapter(claudeAdapter);
registerAdapter(codexAdapter);
registerAdapter(grokAdapter);
