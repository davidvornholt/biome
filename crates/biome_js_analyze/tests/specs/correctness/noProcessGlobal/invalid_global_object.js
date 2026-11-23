/* should generate diagnostics */
globalThis.process.exitCode = 1;

const a = window.process.env;

const b = globalThis["process"].env;

const c = globalThis.window.process.env;

const d = globalThis?.process?.env;

function main() {
    return /* before */ globalThis.process /* after */.env;
}

// The fix would remove the comment.
const e = globalThis /* inside */.process.env;

// The fix would be a syntax error.
delete globalThis.process;

// The fix would read the parameter.
function shadowed(process) {
    return globalThis.process.env;
}
