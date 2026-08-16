import { cpSync, chmodSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
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

// Packages that carry a platform suffix in their *name* are already filtered by
// npm's os/cpu fields, so only two layouts ship foreign binaries: prebuildify's
// `prebuilds/<platform>-<arch>/`, and koffi's `<libc>_<arch>/` beside the glibc
// build. Both are dead weight, and the musl one is worse than that: linuxdeploy
// resolves the dependencies of every ELF in the AppDir, and its unsatisfiable
// libc.musl-x86_64.so.1 fails the AppImage bundle outright.
const KEEP = new Set([
  `${process.platform}-${process.arch}`,
  `${process.platform}_${process.arch}`,
]);

// A name is foreign when its leading platform/libc token is not ours, so
// `linuxmusl-x64` and `musl_x64` are pruned on Linux while `linux_x64` stays.
function isForeignPrebuild(name) {
  const [token] = name.split(/[-_]/);
  return /^(darwin|linux|linuxmusl|win32|android|freebsd|musl)$/.test(token) && !KEEP.has(name);
}

function prunePrebuilds(dir, insidePrebuilds = false) {
  const removed = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    const path = join(dir, entry.name);
    // Only prune where the layout says the directory name selects a platform:
    // inside a prebuilds/prebuilt directory, or a musl sibling of a real build.
    if ((insidePrebuilds && isForeignPrebuild(entry.name)) || /^musl[-_]/.test(entry.name)) {
      removed.push(path);
      rmSync(path, { recursive: true, force: true });
    } else {
      removed.push(...prunePrebuilds(path, entry.name === "prebuilds" || entry.name === "prebuilt"));
    }
  }
  return removed;
}

const pruned = prunePrebuilds(join(dshDir, "node_modules"));
for (const path of pruned) {
  console.log(`Pruned foreign prebuild ${path.slice(dshDir.length + 1)}`);
}

console.log(`Prepared runtime node at ${nodeDest}`);
console.log(`Prepared dsh ${spec} at ${dshDir}`);
console.log(`Pruned ${pruned.length} foreign prebuild director${pruned.length === 1 ? "y" : "ies"}`);
