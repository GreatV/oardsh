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

function lockScopedName(path) {
  const match = path.match(/(?:^|\/)node_modules\/(@deepseek-ai\/[^/]+)$/);
  return match ? match[1] : null;
}

function deepseekPinsFromLock(lockfile) {
  const chosen = new Map();
  for (const [path, pkg] of Object.entries(lockfile.packages ?? {})) {
    if (!pkg.version) continue;
    const name = lockScopedName(path);
    if (!name) continue;
    const depth = path.split("node_modules/").length - 1;
    const prev = chosen.get(name);
    if (!prev || depth < prev.depth) {
      chosen.set(name, { version: pkg.version, depth });
    } else if (depth === prev.depth && pkg.version !== prev.version) {
      throw new Error(
        `package-lock.json pins ${name} to both ${prev.version} and ${pkg.version}`,
      );
    }
  }
  return Object.fromEntries([...chosen].map(([name, { version }]) => [name, version]));
}

// dsh's own package.json uses ^ ranges. A sibling @deepseek-ai/* rc can
// publish before its peers, and a lockless install then asks for a version
// that is not on the registry. Force every scoped package the lock names —
// including nested copies that never got hoisted — to that locked version.
const deepseekPins = deepseekPinsFromLock(lock);

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
      overrides: deepseekPins,
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
// koffi names its two Linux builds after the libc, and picks between them by
// reading the ELF interpreter of the node running it — the very binary this
// script bundles. Detect the same thing, or a musl host would lose the only
// build it can load.
const isMuslHost =
  process.platform === "linux" && !process.report?.getReport?.()?.header?.glibcVersionRuntime;
const libc = process.platform === "linux" && isMuslHost ? "musl" : process.platform;

const KEEP = new Set([
  `${process.platform}-${process.arch}`,
  `${process.platform}_${process.arch}`,
  `${libc}-${process.arch}`,
  `${libc}_${process.arch}`,
]);

// A name is foreign when its leading platform/libc token is not ours. Matching
// the token rather than a prefix keeps `linuxmusl-x64` foreign on glibc.
function isForeignPrebuild(name) {
  const [token] = name.split(/[-_]/);
  return /^(darwin|linux|linuxmusl|win32|android|freebsd|musl)$/.test(token) && !KEEP.has(name);
}

function prunePrebuilds(dir, insidePrebuilds = false) {
  const entries = readdirSync(dir, { withFileTypes: true }).filter((entry) => entry.isDirectory());
  // Outside a prebuilds directory the name is only a selector if our own build
  // sits beside it, so we can never delete the last one standing.
  const oursIsHere = entries.some((entry) => KEEP.has(entry.name));
  const removed = [];
  for (const entry of entries) {
    const path = join(dir, entry.name);
    if (isForeignPrebuild(entry.name) && (insidePrebuilds || oursIsHere)) {
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
