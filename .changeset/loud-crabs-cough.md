---
"@biomejs/biome": patch
---

Fixed [`noProcessGlobal`](https://biomejs.dev/linter/rules/no-process-global/): `biome lint --write` and `biome check --write` no longer hang on a file that reads `process` from the global object, as in `globalThis.process` or `window.process`. The fix only added an import, which left the diagnostic in place. It now also replaces the access with the imported `process`:

```diff
- globalThis.process.exitCode = 1;
+ import process from "node:process";
+ process.exitCode = 1;
```
