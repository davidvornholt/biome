---
"@biomejs/biome": patch
---

Fixed where the safe fix of [`noProcessGlobal`](https://biomejs.dev/linter/rules/no-process-global/) adds `import process from "node:process";`. When `process` is used inside a TypeScript `namespace`, the import is now added at the top level of the file instead of inside the namespace, where TypeScript rejects it. If the file declares a namespace and has no imports or exports, the fix isn't offered in that file, because the import would make the global namespace local to the file. A `biome-ignore` comment above the first statement of the file now stays with that statement instead of ending up above the new import.
