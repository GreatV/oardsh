import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, it } from "node:test";

/**
 * A transcription goes stale silently, so these read the dsh actually installed
 * and check the shapes the plugin reaches for are still in it. An update that
 * moves dsh out from under the injections fails here, in CI, before a release.
 */

const require = createRequire(import.meta.url);
const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");

/// npm may hoist a UI package to the top or nest it under dsh or dsh-web-app.
function dshClient(name) {
  const candidates = [
    `node_modules/@deepseek-ai/${name}/lib/client.js`,
    `node_modules/@deepseek-ai/dsh/node_modules/@deepseek-ai/${name}/lib/client.js`,
    `node_modules/@deepseek-ai/dsh-web-app/node_modules/@deepseek-ai/${name}/lib/client.js`,
  ];
  for (const candidate of candidates) {
    const path = join(root, candidate);
    if (existsSync(path)) return readFileSync(path, "utf8");
  }
  throw new Error(`dsh's ${name} client was not found; looked in:\n  ${candidates.join("\n  ")}`);
}

describe("the dsh this build injects into", () => {
  const source = dshClient("dsh-client-ui-conversation");

  it("is the release the plugin was written against", () => {
    const installed = require("@deepseek-ai/dsh/package.json").version;
    const built = /const DSH_VERSION = "([^"]+)"/.exec(
      readFileSync(join(root, "packages/oardsh-dsh-plugin/lib/client.js"), "utf8"),
    )?.[1];
    assert.equal(
      installed,
      built,
      `dsh moved to ${installed} but the injections were checked against ${built}. ` +
        "Walk the contracts below against the new markup, then pin the new version in package.json.",
    );
  });

  /// Each entry is one thing the plugin reaches for, named the way the runtime
  /// guardrail names it, so a failure here and a warning in the app read alike.
  const CONTRACTS = [
    ["context.ring", /viewBox: "0 0 14 14"/, "the 14px gauge the hover-open effect recognises"],
    ["context.ring", /"aria-haspopup": "dialog"/, "the gauge sits behind a dialog trigger"],
    ["context.ring", /ContextMeter_module_css_default\.track/, "gauge circle 1 of 2: the track"],
    ["context.ring", /ContextMeter_module_css_default\.fill/, "gauge circle 2 of 2: the fill the arcs replace"],
    ["context.panel", /role: "dialog"/, "the panel the extras are written into"],
    ["context.buckets", /ContextMeter_module_css_default\.rows/, "the dl of per-bucket readings"],
    ["context.figures", /formatTokens\(context\.usedTokens\)/, "the `~used / total` figure the free row is derived from"],
    ["context.bar", /ContextMeter_module_css_default\.segment/, "the stacked bar the ring is tinted from"],
    ["stats.strip", /children: "\|"/, "the aria-hidden separator the stats line is found by"],
  ];

  for (const [id, pattern, what] of CONTRACTS) {
    it(`still renders ${what} (${id})`, () => {
      assert.match(source, pattern);
    });
  }

  /// The splitter turns each of these into a term and a reading. A new group
  /// shape is not a break - it falls back to a full-width line - but it is a
  /// reason to look, so the list is asserted rather than assumed.
  it("still writes its stats as the groups the splitter reads", () => {
    for (const key of ["stats.counts", "stats.llm", "stats.toolCall", "stats.tokensPerSecond", "stats.cacheHit", "stats.tokens"]) {
      assert.match(source, new RegExp(`"${key.replace(".", "\\.")}":`), `${key} is gone from dsh's stats line`);
    }
  });
});
