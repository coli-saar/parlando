import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";

const packageDirectory = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const cacheDirectory = path.resolve(packageDirectory, "../.local/npm-cache");
const output = execFileSync(
  "npm",
  ["pack", "--dry-run", "--json", "--cache", cacheDirectory],
  { cwd: packageDirectory, encoding: "utf8" }
);
const jsonStart = output.indexOf("[\n");
if (jsonStart < 0) throw new Error(`npm pack did not produce a JSON file list:\n${output}`);
const packed = JSON.parse(output.slice(jsonStart));
const files = packed[0].files.map((file) => file.path);

/** Fails package verification with one stable diagnostic. */
function requireFile(file) {
  if (!files.includes(file)) throw new Error(`published package is missing ${file}`);
}

for (const required of [
  "package.json",
  "README.md",
  "dist/index.js",
  "dist/index.d.ts",
  "dist/react.js",
  "dist/react.d.ts",
  "dist/audio/captureWorklet.js",
  "dist/audio/playbackWorklet.js"
]) {
  requireFile(required);
}
for (const file of files) {
  if (/\.test\.|(^|\/)src\/|\.private\.|node_modules/.test(file)) {
    throw new Error(`published package contains forbidden development file ${file}`);
  }
}
