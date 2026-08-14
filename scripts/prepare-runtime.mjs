import { cpSync, chmodSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { execFileSync } from "node:child_process";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const runtimeDir = join(root, "src-tauri/resources/runtime");
const dshDir = join(root, "src-tauri/resources/dsh");
const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
const spec = pkg.dependencies["@deepseek-ai/dsh"];

if (!spec) {
  throw new Error("package.json is missing dependency @deepseek-ai/dsh");
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
writeFileSync(
  join(dshDir, "package.json"),
  `${JSON.stringify(
    {
      name: "oardsh-dsh-runtime",
      private: true,
      dependencies: {
        "@deepseek-ai/dsh": spec,
      },
    },
    null,
    2,
  )}\n`,
);

execFileSync("npm", ["install", "--omit=dev"], {
  cwd: dshDir,
  stdio: "inherit",
  env: process.env,
});

console.log(`Prepared runtime node at ${nodeDest}`);
console.log(`Prepared dsh ${spec} at ${dshDir}`);
