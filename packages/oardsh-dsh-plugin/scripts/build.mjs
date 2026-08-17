import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import sharp from "sharp";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const template = readFileSync(join(root, "src/client.template.js"), "utf8");
const messages = {
  en: JSON.parse(readFileSync(join(root, "locales/en.json"), "utf8")),
  zh: JSON.parse(readFileSync(join(root, "locales/zh-CN.json"), "utf8")),
};

/// The app icon as an alpha mask, so `currentColor` can paint it: a baked-in
/// colour would vanish in one of dsh's themes. The inset drops the plate.
async function brandMask() {
  const size = 32;
  const inset = 12;
  const side = 128 - inset * 2;
  const alpha = await sharp(join(root, "../../src-tauri/icons/128x128.png"))
    .flatten({ background: "#ffffff" })
    .extract({ left: inset, top: inset, width: side, height: side })
    .greyscale()
    .negate()
    .resize(size, size, { fit: "contain", background: "#000000" })
    .raw()
    .toBuffer();
  const rgba = Buffer.alloc(size * size * 4);
  for (let index = 0; index < size * size; index += 1) {
    rgba[index * 4 + 3] = alpha[index];
  }
  const png = await sharp(rgba, { raw: { width: size, height: size, channels: 4 } })
    .png({ compressionLevel: 9, palette: true })
    .toBuffer();
  return `data:image/png;base64,${png.toString("base64")}`;
}

/// The dsh release the injections were written against. Baked in so a lapsed
/// contract can name it, and read from the root manifest rather than the
/// prepared runtime, which does not exist yet on a clean checkout.
const pinned = JSON.parse(readFileSync(join(root, "../../package.json"), "utf8"));
const dshVersion = pinned.dependencies["@deepseek-ai/dsh"].replace(/^[\^~]/, "");

const output = template
  .replace("__OARDSH_MESSAGES__", JSON.stringify(messages))
  .replace("__OARDSH_DSH_VERSION__", dshVersion)
  .replace("__OARDSH_MARK__", await brandMask());
mkdirSync(join(root, "lib"), { recursive: true });
writeFileSync(join(root, "lib/client.js"), output);
