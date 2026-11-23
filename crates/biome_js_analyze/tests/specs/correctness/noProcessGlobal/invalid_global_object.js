/* should generate diagnostics */
globalThis.process.exitCode = 1;

const a = window.process.env;

const b = globalThis["process"].env;

const c = globalThis.window.process.env;

const d = globalThis?.process?.env;

const e = typeof globalThis.process;
