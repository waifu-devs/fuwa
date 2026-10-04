// Builds dist/esm (ES modules) and dist/cjs (CommonJS), both with types, from src/.
import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const { version } = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
const versionFile = join(root, "src/version.ts");
const stamped = `// Set by scripts/build.mjs from package.json; release.yml stamps the tag's version there.\nexport const SDK_VERSION = ${JSON.stringify(version)};\n`;
if (readFileSync(versionFile, "utf8") !== stamped) writeFileSync(versionFile, stamped);

rmSync(join(root, "dist"), { recursive: true, force: true });
const tsc = (...args) => execFileSync("tsc", args, { cwd: root, stdio: "inherit", shell: process.platform === "win32" });

// The package is "type": "module", so NodeNext emits ES modules here...
tsc("-p", "tsconfig.json");
// ...and CommonJS under a folder whose package.json says so.
mkdirSync(join(root, "dist/cjs"), { recursive: true });
writeFileSync(join(root, "dist/cjs/package.json"), '{ "type": "commonjs" }\n');
tsc("-p", "tsconfig.json", "--module", "commonjs", "--moduleResolution", "bundler", "--verbatimModuleSyntax", "false", "--outDir", "dist/cjs");
