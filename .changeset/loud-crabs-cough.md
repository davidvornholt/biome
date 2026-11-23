---
"@biomejs/biome": patch
---

Fixed [`noProcessGlobal`](https://biomejs.dev/linter/rules/no-process-global/): `biome lint --write` and `biome check --write` no longer hang on a file that reads `process` from the global object, as in `globalThis.process` or `window.process`. The rule still reports these accesses, but it no longer offers its fix for them, because adding an import doesn't change what they read.
