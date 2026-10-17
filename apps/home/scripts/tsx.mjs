// Lets `node --test` import .tsx files: TypeScript (already a dev dependency) turns their JSX into react/jsx-runtime
// calls; Node strips the types of .ts/.mts itself. It lives outside test/ so `node --test` does not count it as a test.
// Import it first, then load components with a dynamic import():
//   import "../scripts/tsx.mjs";
//   const { Shell } = await import("../src/components/shell.tsx");
import { readFileSync } from "node:fs";
import { registerHooks } from "node:module";
import { fileURLToPath } from "node:url";
import ts from "typescript";

registerHooks({
  // Bare subpaths of CommonJS packages without an exports map (`next/navigation`) resolve only with their extension.
  resolve(specifier, context, nextResolve) {
    try {
      return nextResolve(specifier, context);
    } catch (e) {
      if (e?.code !== "ERR_MODULE_NOT_FOUND" || /^[./]|\.js$/.test(specifier)) throw e;
      return nextResolve(`${specifier}.js`, context);
    }
  },
  load(url, context, nextLoad) {
    if (!url.startsWith("file:") || !url.endsWith(".tsx")) return nextLoad(url, context);
    const fileName = fileURLToPath(url);
    const { outputText } = ts.transpileModule(readFileSync(fileName, "utf8"), {
      fileName,
      compilerOptions: { jsx: ts.JsxEmit.ReactJSX, module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 },
    });
    return { format: "module", source: outputText, shortCircuit: true };
  },
});
