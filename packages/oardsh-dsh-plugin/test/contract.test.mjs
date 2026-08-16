import assert from "node:assert/strict";
import { after, describe, it } from "node:test";
import { boot, frame, hover, rowsOf } from "./harness.mjs";
import { BUCKETS, STATS_EN, STATS_ZH, mountContextMeter, mountStatsLine } from "./dsh-fixture.mjs";

/**
 * The plugin decorates dsh surfaces that no slot exposes, so the failure to
 * defend against is not a crash: it is the extras quietly disappearing after a
 * dsh bump, unnoticed until a user reports an empty panel.
 */

/// Open the meter by hovering it, the way the plugin arms it.
async function openPanel(app, meter) {
  hover(app.window, meter.svg);
  await frame(app.window);
  return meter.root.querySelector('[role="dialog"]');
}

/// Leave the meter: anything outside the meter's root closes it.
async function leave(app) {
  hover(app.window, app.document.getElementById("composer"));
  await frame(app.window);
}

function mount({ locale = "en", stats = STATS_EN } = {}) {
  const app = boot({ locale });
  const meter = mountContextMeter(app.document);
  app.document.getElementById("composer").append(mountStatsLine(app.document, stats), meter.root);
  app.apply();
  return { app, meter };
}

describe("context meter", () => {
  const { app, meter } = mount();
  after(() => app.dispose());

  it("opens the panel on hover and decorates it", async () => {
    const panel = await openPanel(app, meter);
    assert.ok(panel, "hovering the ring opens dsh's panel");
    assert.equal(meter.trigger.getAttribute("aria-expanded"), "true");

    const shares = [...panel.querySelectorAll("[data-oardsh-share]")];
    assert.equal(shares.length, BUCKETS.length, "every bucket reading gains a share");
    assert.match(shares[0].textContent, /%$/);
    assert.ok(panel.querySelector("[data-oardsh-free]"), "the free remainder joins dsh's own list");
    assert.ok(panel.querySelector("[data-oardsh-context-extra]"), "session stats are mirrored in");
  });

  it("tints the ring with the bar's own colours", async () => {
    const arcs = [...meter.svg.querySelectorAll("[data-oardsh-arc]")];
    assert.equal(arcs.length, BUCKETS.length);
    assert.ok(arcs.every((arc) => arc.getAttribute("stroke-dasharray")));
    const fill = meter.svg.querySelector("circle:not([data-oardsh-arc]) + circle:not([data-oardsh-arc])");
    assert.equal(fill.style.display, "none", "dsh's grey fill gives way to the tinted arcs");
  });

  // The arcs above are circles that outlive the panel, so counting every circle
  // in the gauge stops recognising the ring once it has been decorated: the
  // second hover falls through to dsh's tooltip and the panel "works once".
  it("still opens and decorates on every later hover", async () => {
    await leave(app);
    assert.equal(meter.trigger.getAttribute("aria-expanded"), "false", "leaving closes the panel");

    for (const attempt of [2, 3]) {
      const panel = await openPanel(app, meter);
      assert.ok(panel, `hover ${attempt} opens the panel again`);
      assert.equal(
        panel.querySelectorAll("[data-oardsh-share]").length,
        BUCKETS.length,
        `hover ${attempt} decorates the fresh panel`,
      );
      assert.ok(panel.querySelector("[data-oardsh-context-extra]"), `hover ${attempt} mirrors the stats`);
      await leave(app);
    }
  });

  it("reports no lapsed contracts against the dsh it was built for", () => {
    assert.deepEqual(app.lapsed(), []);
    assert.match(app.window.__OARDSH__.dsh, /^\d+\.\d+\.\d+/);
    assert.deepEqual(app.warnings, []);
  });
});

describe("mirrored session stats", () => {
  for (const [name, groups, expected] of [
    ["English", STATS_EN, [["turns", "2"], ["steps", "5"], ["LLM", "35m38s"], ["Tool call", "20m38s"], ["TTFT avg", "9.9s"], ["tok/s", "97"], ["Cache hit", "99%"], ["Input", "13.6M tok"], ["Output", "0.1M tok"]]],
    ["Chinese", STATS_ZH, [["轮", "2"], ["步", "5"], ["LLM", "35m38s"], ["工具调用", "20m38s"], ["首 token 平均", "9.9s"], ["tok/s", "97"], ["缓存命中", "99%"], ["输入", "13.6M tok"], ["输出", "0.1M tok"]]],
  ]) {
    it(`splits ${name} stats into a term and a reading, like every other row`, async () => {
      const { app, meter } = mount({ stats: groups });
      const panel = await openPanel(app, meter);
      const extra = panel.querySelector("[data-oardsh-context-extra]");
      assert.deepEqual(rowsOf(extra), expected);
      // Same shape as dsh's bucket rows above them: term left, reading right.
      for (const row of extra.querySelectorAll("dl > div")) {
        assert.equal(row.querySelector("dd").dataset.wide, undefined, "readings stay in the right column");
      }
      app.dispose();
    });
  }

  it("leaves the stats in dsh's own line when the preference says so", async () => {
    const app = boot();
    const meter = mountContextMeter(app.document);
    const strip = mountStatsLine(app.document);
    app.document.getElementById("composer").append(strip, meter.root);
    app.apply();
    await frame(app.window);
    assert.ok("oardshStatsHidden" in strip.dataset, "the panel placement hides dsh's line by default");
    app.dispose();
  });
});

describe("when dsh's markup moves", () => {
  it("names the lapsed contract instead of decorating a gauge it misread", async () => {
    const app = boot();
    const meter = mountContextMeter(app.document);
    // dsh redraws the gauge with a third arc: the plugin must not assume the
    // two circles it knows, and must not paint over a control it cannot read.
    const extra = app.document.createElementNS("http://www.w3.org/2000/svg", "circle");
    meter.svg.appendChild(extra);
    app.document.getElementById("composer").append(meter.root);
    app.apply();

    hover(app.window, meter.svg);
    await frame(app.window);
    assert.equal(meter.root.querySelector('[role="dialog"]'), null, "no panel is forced open");
    assert.deepEqual(app.lapsed(), ["context.ring"]);
    assert.match(app.warnings.join("\n"), /context\.ring/);
    assert.match(app.warnings.join("\n"), /Built against dsh/);
    app.dispose();
  });

  it("survives a panel with none of the shapes it reads", async () => {
    const app = boot();
    const meter = mountContextMeter(app.document);
    meter.trigger.addEventListener("click", () => {
      // A panel that opens but carries no bar, no figures and no buckets.
      const dialog = meter.root.querySelector('[role="dialog"]');
      if (dialog) dialog.innerHTML = "<p>context</p>";
    });
    app.document.getElementById("composer").append(meter.root);
    app.apply();

    hover(app.window, meter.svg);
    await frame(app.window);
    const panel = meter.root.querySelector('[role="dialog"]');
    assert.ok(panel, "dsh's own panel still opens");
    assert.equal(panel.querySelector("[data-oardsh-free]"), null, "nothing is invented from missing figures");
    assert.deepEqual(
      app.lapsed().sort(),
      ["context.bar", "context.buckets", "context.figures"],
      "each surface that went missing is named once",
    );
    app.dispose();
  });

  it("ignores dialog triggers that are not the context ring", async () => {
    const app = boot();
    const other = app.document.createElement("button");
    other.setAttribute("aria-haspopup", "dialog");
    other.innerHTML = '<svg viewBox="0 0 16 16"><circle cx="8" cy="8" r="6"></circle></svg>';
    app.document.getElementById("composer").append(other);
    app.apply();

    hover(app.window, other.querySelector("circle"));
    await frame(app.window);
    assert.equal(other.getAttribute("aria-expanded"), null, "an unrelated menu is never clicked open");
    assert.deepEqual(app.lapsed(), [], "and it is not mistaken for drift");
    app.dispose();
  });
});
