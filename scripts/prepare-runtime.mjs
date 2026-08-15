import { cpSync, chmodSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { execFileSync } from "node:child_process";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const runtimeDir = join(root, "src-tauri/resources/runtime");
const dshDir = join(root, "src-tauri/resources/dsh");
const pluginSource = join(root, "packages/oardsh-dsh-plugin");
const pluginDest = join(dshDir, "plugins/oardsh-dsh-plugin");
// This directory installs with no committed lockfile, so a range would resolve
// to whatever is newest on the release runner. Take what the root lock pins.
const lock = JSON.parse(readFileSync(join(root, "package-lock.json"), "utf8"));
const spec = lock.packages?.["node_modules/@deepseek-ai/dsh"]?.version;

if (!spec) {
  throw new Error("package-lock.json does not pin @deepseek-ai/dsh; run npm install");
}

mkdirSync(runtimeDir, { recursive: true });
const nodeName = process.platform === "win32" ? "node.exe" : "node";
const nodeDest = join(runtimeDir, nodeName);
cpSync(process.execPath, nodeDest);
if (process.platform !== "win32") {
  chmodSync(nodeDest, 0o755);
}

rmSync(dshDir, { recursive: true, force: true });
mkdirSync(dshDir, { recursive: true });
execFileSync(process.execPath, [join(pluginSource, "scripts/build.mjs")], {
  cwd: root,
  stdio: "inherit",
});
cpSync(pluginSource, pluginDest, { recursive: true });
writeFileSync(
  join(dshDir, "package.json"),
  `${JSON.stringify(
    {
      name: "oardsh-dsh-runtime",
      private: true,
      dependencies: {
        "@deepseek-ai/dsh": spec,
        "@oardsh/dsh-plugin": "file:plugins/oardsh-dsh-plugin",
      },
    },
    null,
    2,
  )}\n`,
);

// On Windows the bare name misses the PATHEXT lookup, and since CVE-2024-27980
// Node refuses to execFile npm.cmd at all. npm points npm_execpath at its own
// JS entry, which runs on every platform without a shell.
const npmEntry = process.env.npm_execpath?.endsWith(".js")
  ? process.env.npm_execpath
  : null;
const args = ["install", "--omit=dev"];
execFileSync(npmEntry ? process.execPath : "npm", npmEntry ? [npmEntry, ...args] : args, {
  cwd: dshDir,
  stdio: "inherit",
  env: process.env,
  shell: !npmEntry && process.platform === "win32",
});

console.log(`Prepared runtime node at ${nodeDest}`);
console.log(`Prepared dsh ${spec} at ${dshDir}`);
