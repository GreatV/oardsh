import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { JSDOM } from "jsdom";
import * as React from "react";

/**
 * Boots the built plugin the way dsh does: `lib/client.js` hands a factory to
 * `window.__ModuleLoader__`, cordis calls it with a `require`, and `apply` gets
 * a context of the services named in `inject`. The built file is what ships, so
 * a test against the template would skip the substitutions.
 */
const here = dirname(fileURLToPath(import.meta.url));
const CLIENT = join(here, "..", "lib", "client.js");

export function boot({ locale = "en" } = {}) {
  const dom = new JSDOM("<!doctype html><body><main id='composer'></main></body>", {
    runScripts: "outside-only",
    pretendToBeVisual: true,
  });
  const { window } = dom;
  const warnings = [];
  window.console = { ...console, warn: (...args) => warnings.push(args.join(" ")) };

  let loaded = null;
  window.__ModuleLoader__ = { load: (mod) => (loaded = mod) };
  window.eval(readFileSync(CLIENT, "utf8"));
  if (loaded === null) throw new Error("the plugin never registered with the module loader");

  const exported = loaded.factory((name) => {
    if (name === "react") return React;
    throw new Error(`the plugin asked for an unexpected module: ${name}`);
  });

  const settings = new Map();
  const disposers = [];
  const ctx = {
    locale: {
      register: () => {},
      bind: () => (key) => key,
      getLocale: () => ({ active: locale }),
    },
    settingsScope: {
      bind: () => ({
        getSnapshot: () => ({ value: Object.fromEntries(settings) }),
        set: (key, value) => settings.set(key, value),
        subscribe: () => () => {},
      }),
    },
    slots: {
      inject: (_name, register) => register(),
      register: (meta, render) => ({ meta, render }),
    },
    effect: (run) => {
      const dispose = run();
      if (typeof dispose === "function") disposers.push(dispose);
    },
  };

  return {
    window,
    document: window.document,
    exported,
    ctx,
    warnings,
    /// Copied out of the jsdom realm: an array built in there has a different
    /// `Array` prototype, which `deepStrictEqual` refuses to match.
    lapsed: () => [...window.__OARDSH__.lapsed()],
    apply: () => exported.apply(ctx),
    dispose: () => {
      for (const dispose of disposers.splice(0)) dispose();
      window.close();
    },
  };
}

/** dsh renders on the next frame; so does the plugin's first decoration pass. */
export const frame = (window) =>
  new Promise((resolve) => window.requestAnimationFrame(() => window.requestAnimationFrame(resolve)));

/** A pointer landing on `target`, as the capture-phase listener sees it. */
export function hover(window, target) {
  target.dispatchEvent(new window.MouseEvent("mouseover", { bubbles: true, view: window }));
}

/** Every row the panel shows, as `[term, reading]` pairs. */
export function rowsOf(panel) {
  return [...panel.querySelectorAll("dl > div")].map((row) => [
    row.querySelector("dt")?.textContent?.trim() ?? "",
    row.querySelector("dd")?.textContent?.trim() ?? "",
  ]);
}
