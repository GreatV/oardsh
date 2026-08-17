window.__ModuleLoader__.load({
  id: "@oardsh/dsh-plugin",
  factory: (require) => {
    var module = { exports: {} };
    var exports = module.exports;
    const React = require("react");
    const { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } = React;
    const h = React.createElement;
    const messages = __OARDSH_MESSAGES__;
    /// Alpha mask of the app icon, painted with currentColor. It marks every
    /// surface oardsh injects, so none of them reads as stock dsh.
    const MARK = "__OARDSH_MARK__";
    const NS = "oardsh.desktop";
    /// dsh's markup is not a public API and the preview ships breaking changes,
    /// so every lookup into it is a contract that can lapse. Losing a surface is
    /// the right failure; losing it silently is not, so name the one that broke
    /// and the dsh it was written against.
    const DSH_VERSION = "__OARDSH_DSH_VERSION__";
    const lapsed = new Set();
    const contract = (id, value) => {
      const held = value !== null && value !== undefined && value !== false;
      if (!held && !lapsed.has(id)) {
        lapsed.add(id);
        console.warn(
          `[oardsh] dsh markup contract "${id}" no longer matches. Built against dsh ${DSH_VERSION}; ` +
            "the desktop extras on that surface stay off until it is updated.",
        );
      }
      return value;
    };
    // Diagnostics for a bug report: what lapsed, and what it was built against.
    window.__OARDSH__ = { dsh: DSH_VERSION, lapsed: () => [...lapsed] };
    const RANGES = [7, 30];
    const SERIES_SLOTS = 8;
    const invoke = (command, args = {}) => {
      const nativeInvoke = window.__TAURI_INTERNALS__?.invoke;
      if (typeof nativeInvoke !== "function") return Promise.reject(new Error("native-unavailable"));
      return nativeInvoke(command, args);
    };
    // dsh never writes <html lang>, so Intl reads the same snapshot the UI does.
    const intlLocale = (id) => (String(id || "").toLowerCase().startsWith("zh") ? "zh-CN" : "en");
    let readLocale = () => intlLocale(navigator.language);
    const locale = () => readLocale();
    const compact = (value) => new Intl.NumberFormat(locale(), { notation: "compact", maximumFractionDigits: 1 }).format(value || 0);
    const percent = (value) => `${value >= 0.1 || value === 0 ? Math.round(value * 10) / 10 : "<0.1"}%`;
    const dayDate = (day) => { const [y, m, d] = day.split("-").map(Number); return new Date(y, m - 1, d); };
    const dayLabel = (day) => dayDate(day).toLocaleDateString(locale(), { month: "short", day: "numeric" });
    const fullDayLabel = (day) => dayDate(day).toLocaleDateString(locale(), { year: "numeric", month: "short", day: "numeric", weekday: "short" });

    /** One day, as the rows the hover card prints. */
    const dayTip = (day, t) => ({
      title: fullDayLabel(day.day),
      rows: [
        [t("stats.tokens"), compact(day.totalTokens)],
        [t("stats.messages"), String(day.messages)],
        [t("stats.sessions"), String(day.sessions.length)],
        ...day.models.map((entry) => [entry.model, compact(entry.totalTokens)]),
      ],
    });
    /** Anchor the hover card above the mark, kept inside the viewport. */
    const tipAt = (event, content) => {
      const rect = event.currentTarget.getBoundingClientRect();
      return { ...content, x: Math.min(Math.max(rect.left + rect.width / 2, 140), window.innerWidth - 140), y: rect.top - 10 };
    };

    // Slots are assigned per model, never by rank, so dropping one model from a
    // range does not repaint the rest.
    const css = `
      .oardsh-section{max-width:720px;color:var(--dsw-alias-label-primary);display:flex;flex-direction:column;gap:18px;--oardsh-series-1:#2a78d6;--oardsh-series-2:#eb6834;--oardsh-series-3:#1baf7a;--oardsh-series-4:#eda100;--oardsh-series-5:#e87ba4;--oardsh-series-6:#008300;--oardsh-series-7:#4a3aa7;--oardsh-series-8:#e34948;--oardsh-series-other:#8a8a8a;--oardsh-heat-0:#dee2e6;--oardsh-heat-1:#bcd7f5;--oardsh-heat-2:#8bbaeb;--oardsh-heat-3:#5896dd;--oardsh-heat-4:#2a78d6}
      body[data-ds-dark-theme] .oardsh-section{--oardsh-series-1:#3987e5;--oardsh-series-2:#d95926;--oardsh-series-3:#199e70;--oardsh-series-4:#c98500;--oardsh-series-5:#d55181;--oardsh-series-6:#008300;--oardsh-series-7:#9085e9;--oardsh-series-8:#e66767;--oardsh-series-other:#8a8a8a;--oardsh-heat-0:#43454a;--oardsh-heat-1:#2d5a8c;--oardsh-heat-2:#356fb0;--oardsh-heat-3:#3a7cd0;--oardsh-heat-4:#4a90ea}
      .oardsh-heading{display:flex;flex-direction:column;gap:3px}.oardsh-title{font-size:16px;line-height:24px;font-weight:500;margin:0}.oardsh-description,.oardsh-muted{font-size:12px;line-height:18px;color:var(--dsw-alias-label-tertiary);margin:0}
      .oardsh-section-head{display:flex;gap:12px;align-items:center}.oardsh-section-head>div:first-child{min-width:0;flex:1}.oardsh-subtitle{font-size:14px;line-height:22px;font-weight:500;margin:0}
      .oardsh-button{box-sizing:border-box;height:30px;border:1px solid var(--dsw-alias-border-l2);background:transparent;color:var(--dsw-alias-label-primary);border-radius:15px;padding:0 12px;font:inherit;font-size:12px;cursor:pointer}.oardsh-button:hover:not(:disabled){background:var(--dsw-alias-interactive-bg-hover-solid)}.oardsh-button:disabled{opacity:.4;cursor:default}
      .oardsh-switch{display:flex;gap:2px;padding:2px;border:1px solid var(--dsw-alias-border-l2);border-radius:16px;flex:none}.oardsh-switch button{height:26px;padding:0 12px;border:0;border-radius:14px;background:transparent;color:var(--dsw-alias-label-tertiary);font:inherit;font-size:12px;cursor:pointer}.oardsh-switch button:hover{color:var(--dsw-alias-label-primary)}.oardsh-switch button[data-active]{background:var(--dsw-alias-button-ghost-active-fill);color:var(--dsw-alias-label-primary);font-weight:500}
      .oardsh-error{border-radius:8px;background:var(--dsw-alias-interactive-bg-hover-danger);color:var(--dsw-alias-state-error-primary);font-size:12px;line-height:18px;padding:8px 10px}.oardsh-empty{border:1px dashed var(--dsw-alias-border-l3);border-radius:10px;color:var(--dsw-alias-label-tertiary);font-size:12px;padding:18px;text-align:center}
      .oardsh-card{background:var(--dsw-alias-bg-module-platform);border-radius:12px;padding:14px 16px;display:flex;flex-direction:column;gap:12px}
      .oardsh-tiles{display:grid;grid-template-columns:repeat(3,1fr);gap:8px}.oardsh-tile{background:var(--dsw-alias-bg-module-platform);border-radius:12px;padding:12px 14px;display:flex;flex-direction:column;gap:2px;min-width:0}.oardsh-tile-label{font-size:12px;line-height:18px;color:var(--dsw-alias-label-tertiary)}.oardsh-tile strong{font-size:22px;line-height:32px;font-weight:500;font-variant-numeric:tabular-nums}.oardsh-tile-note{font-size:11px;line-height:16px;color:var(--dsw-alias-label-caption);overflow-wrap:anywhere}.oardsh-tile-name{font-size:15px;line-height:26px;font-weight:500;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
      .oardsh-heat-wrap{overflow-x:auto}.oardsh-heatmap{display:grid;grid-template-rows:repeat(7,1fr);grid-auto-flow:column;grid-auto-columns:9px;gap:3px;min-width:min-content}.oardsh-heat-cell{width:9px;height:9px;border-radius:2px;background:var(--oardsh-heat-0)}.oardsh-heat-cell[data-level="1"]{background:var(--oardsh-heat-1)}.oardsh-heat-cell[data-level="2"]{background:var(--oardsh-heat-2)}.oardsh-heat-cell[data-level="3"]{background:var(--oardsh-heat-3)}.oardsh-heat-cell[data-level="4"]{background:var(--oardsh-heat-4)}.oardsh-heat-cell[data-pad]{background:transparent}.oardsh-heat-cell:not([data-pad]):hover{outline:1px solid var(--dsw-alias-label-tertiary);outline-offset:1px}
      .oardsh-scale{display:flex;align-items:center;gap:4px;font-size:11px;color:var(--dsw-alias-label-tertiary)}.oardsh-scale i{width:9px;height:9px;border-radius:2px;display:block}
      .oardsh-plot{position:relative;height:180px;background:repeating-linear-gradient(to top,transparent 0,transparent 43px,var(--dsw-alias-border-l2) 43px,var(--dsw-alias-border-l2) 44px)}.oardsh-peak{font-size:11px;color:var(--dsw-alias-label-caption);flex:none}
      .oardsh-bars{position:absolute;inset:0;display:flex;align-items:flex-end;gap:2px}.oardsh-bar{flex:1;height:100%;display:flex;flex-direction:column-reverse;gap:2px;justify-content:flex-start;border-radius:3px;min-width:0}.oardsh-bar:hover{background:var(--dsw-alias-interactive-bg-hover)}.oardsh-bar i{display:block;width:100%;max-width:56px;margin-inline:auto;min-height:2px}.oardsh-bar i:last-child{border-radius:4px 4px 0 0}
      .oardsh-axis{display:flex;gap:2px;font-size:11px;color:var(--dsw-alias-label-tertiary)}.oardsh-axis span{flex:1;min-width:0;text-align:center;white-space:nowrap}.oardsh-axis span:first-child{text-align:left}.oardsh-axis span:last-child{text-align:right}
      .oardsh-legend{display:flex;flex-wrap:wrap;gap:6px 18px;font-size:12px;color:var(--dsw-alias-label-secondary)}.oardsh-legend span{display:flex;align-items:center;gap:6px}
      .oardsh-dot{width:9px;height:9px;border-radius:50%;flex:none}
      .oardsh-mark{display:inline-block;width:14px;height:14px;flex:none;background:currentColor;-webkit-mask:url("${MARK}") center/contain no-repeat;mask:url("${MARK}") center/contain no-repeat}
      .oardsh-brand{display:flex;align-items:center;gap:7px}.oardsh-brand .oardsh-mark{width:16px;height:16px;color:var(--dsw-alias-label-secondary)}
      .oardsh-general-row{display:flex;align-items:center;gap:10px;padding:16px 0;border-bottom:1px solid var(--dsw-alias-border-l2)}.oardsh-general-row .oardsh-mark{color:var(--dsw-alias-label-tertiary)}.oardsh-general-text{flex:1;min-width:0;padding-right:24px;display:flex;flex-direction:column;gap:2px}.oardsh-general-title{color:var(--dsw-alias-label-primary);font-size:14px;line-height:22px}.oardsh-general-help{color:var(--dsw-alias-label-tertiary);font-size:12px;line-height:18px}
      .oardsh-ctx-brand{display:flex;align-items:center;gap:5px;color:var(--dsw-alias-label-caption);font-size:11px;line-height:16px;margin-bottom:2px}.oardsh-ctx-brand .oardsh-mark{width:11px;height:11px}
      .oardsh-ctx-share{color:var(--dsw-alias-label-tertiary);font-weight:400}
      .oardsh-ctx-swatch{background:var(--meter-tint,var(--dsw-alias-label-tertiary));vertical-align:baseline;border-radius:2px;width:8px;height:8px;margin-right:6px;display:inline-block}
      .oardsh-ctx-extra{margin:8px 0 0;padding-top:8px;border-top:1px solid var(--dsw-alias-border-l2)}.oardsh-ctx-rows{margin:0}.oardsh-ctx-rows>div{display:flex;justify-content:space-between;align-items:center;gap:12px;padding:2px 0}.oardsh-ctx-rows dt{color:var(--dsw-alias-label-secondary);white-space:nowrap;flex:none}.oardsh-ctx-rows dt:empty{display:none}.oardsh-ctx-rows dd{margin:0;color:var(--dsw-alias-label-primary);font-variant-numeric:tabular-nums;text-align:right}.oardsh-ctx-rows dd[data-wide]{color:var(--dsw-alias-label-primary);flex:1;text-align:left}
      [data-oardsh-stats-hidden]{display:none!important}
      .oardsh-tip{position:fixed;z-index:2147483000;transform:translate(-50%,-100%);pointer-events:none;min-width:150px;max-width:260px;border:1px solid var(--dsw-alias-border-l2);background:var(--dsw-specific-menu,var(--dsw-alias-bg-layer-1));box-shadow:var(--dsw-shadow-lv3,0 6px 20px rgba(0,0,0,.18));border-radius:10px;padding:9px 11px;font-size:12px;line-height:18px;color:var(--dsw-alias-label-secondary)}
      .oardsh-tip-title{color:var(--dsw-alias-label-primary);font-weight:500;margin-bottom:4px}.oardsh-tip-rows{margin:0}.oardsh-tip-rows>div{display:flex;justify-content:space-between;gap:14px}.oardsh-tip-rows dt{min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.oardsh-tip-rows dd{margin:0;color:var(--dsw-alias-label-primary);font-variant-numeric:tabular-nums;flex:none}
      .oardsh-donut-row{display:flex;align-items:center;gap:20px}.oardsh-donut{position:relative;flex:none;width:160px;height:160px}.oardsh-donut-center{position:absolute;inset:0;display:flex;flex-direction:column;align-items:center;justify-content:center;gap:1px}.oardsh-donut-center strong{font-size:19px;line-height:26px;font-weight:500;font-variant-numeric:tabular-nums}.oardsh-donut-center span{font-size:11px;color:var(--dsw-alias-label-tertiary)}
      .oardsh-list{display:flex;flex-direction:column;margin:0;padding:0;list-style:none;flex:1;min-width:0}.oardsh-row{min-height:44px;display:flex;align-items:center;gap:10px;border-bottom:1px solid var(--dsw-alias-border-l2);padding:6px 0}.oardsh-row:last-child{border-bottom:0}.oardsh-row-main{min-width:0;flex:1}.oardsh-row-title{font-size:13px;line-height:20px;font-weight:500;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.oardsh-row-note{font-size:11px;line-height:16px;color:var(--dsw-alias-label-caption)}.oardsh-row-value{font-size:13px;font-variant-numeric:tabular-nums;color:var(--dsw-alias-label-secondary);flex:none}
      @media(max-width:650px){.oardsh-tiles{grid-template-columns:1fr 1fr}.oardsh-donut-row{flex-direction:column;align-items:stretch}.oardsh-donut{align-self:center}}
    `;
    if (!document.querySelector('style[data-plugin-css="@oardsh/dsh-plugin"]')) {
      const style = document.createElement("style");
      style.dataset.plugin = "@oardsh/dsh-plugin";
      style.dataset.pluginCss = "@oardsh/dsh-plugin";
      style.textContent = css;
      document.head.appendChild(style);
    }

    const colorFor = (index) => (index < SERIES_SLOTS ? `var(--oardsh-series-${index + 1})` : "var(--oardsh-series-other)");

    function Heading({ title, description, action }) {
      return h("div", { className: "oardsh-section-head" },
        h("div", null, h("h3", { className: "oardsh-subtitle" }, title), description && h("p", { className: "oardsh-muted" }, description)), action);
    }
    function Tile({ label, value, note, name }) {
      return h("div", { className: "oardsh-tile" },
        h("span", { className: "oardsh-tile-label" }, label),
        name ? h("span", { className: "oardsh-tile-name", title: name }, name) : h("strong", null, value),
        note && h("span", { className: "oardsh-tile-note", title: note }, note));
    }

    // dsh's context ring is hardcoded into the input bar, not exposed through a
    // slot, so the plugin enhances the rendered element instead. Every lookup
    // below is defensive: changed dsh markup loses the extras, not the page.
    /** dsh renders token counts as `517`, `12.2K`, `1.2M`, locale-independent. */
    const parseTokens = (text) => {
      const match = /(-?[\d.]+)\s*([KM])?/.exec(String(text || "").replace(/[~\s,]/g, ""));
      if (!match) return null;
      const value = Number(match[1]);
      if (!Number.isFinite(value)) return null;
      return value * (match[2] === "M" ? 1e6 : match[2] === "K" ? 1e3 : 1);
    };
    const formatTokens = (value) => {
      const scaled = (part) => (part >= 100 ? String(Math.round(part)) : String(Math.round(part * 10) / 10));
      if (value < 1e3) return String(Math.round(value));
      return value < 1e6 ? `${scaled(value / 1e3)}K` : `${scaled(value / 1e6)}M`;
    };
    const share = (value, total) => `${Math.round((value / total) * 1000) / 10}%`;

    /**
     * dsh joins its stats into groups ("LLM 35m38s · 工具调用 20m38s") that the
     * panel has to show as the term/reading pairs every other row uses. Split at
     * the first digit-leading word; a reading-first stat ("2 轮", "97 tok/s") has
     * no term, so its trailing unit becomes one.
     */
    const splitStat = (text) => {
      const words = String(text).trim().split(/\s+/).filter(Boolean);
      const at = words.findIndex((word) => /^[\d.]/.test(word));
      if (at < 0) return { label: words.join(" "), value: "" };
      if (at > 0) return { label: words.slice(0, at).join(" "), value: words.slice(at).join(" ") };
      const unit = words.slice(1).join(" ");
      return unit ? { label: unit, value: words[0] } : { label: "", value: words[0] };
    };
    const statRows = (group) => group.split("·").map((item) => splitStat(item)).filter((row) => row.label || row.value);

    /**
     * Where the session stats belong: the context panel (default) or dsh's own
     * line under the composer. Kept out of React state because the DOM effects
     * read it too; the durable copy lives in dsh's settings document.
     */
    const preferences = { statsInPanel: true, write: null };
    const preferenceListeners = new Set();
    const notifyPreferences = () => { for (const listener of preferenceListeners) listener(); };

    /**
     * dsh's stats strip, found by its "|" separator and then remembered: this
     * runs on DOM mutations, too often to rescan every span each frame.
     */
    let stripCache = null;
    const statsStrip = () => {
      if (stripCache?.isConnected) return stripCache;
      stripCache = null;
      for (const separator of document.querySelectorAll('span[aria-hidden="true"]')) {
        if (separator.textContent?.trim() !== "|") continue;
        const line = separator.parentElement;
        if (line && statsParts(line).length > 1) { stripCache = line; break; }
      }
      return stripCache;
    };
    const statsParts = (line) => (line.textContent || "").split("|").map((part) => part.trim()).filter(Boolean);
    /** Only mirror what we actually hid, so the two can never both be visible. */
    const mirroredStats = () => {
      const strip = statsStrip();
      return strip && "oardshStatsHidden" in strip.dataset ? statsParts(strip) : [];
    };
    const applyStatsPlacement = () => {
      const strip = statsStrip();
      if (!strip) return;
      const hidden = "oardshStatsHidden" in strip.dataset;
      // Writing an unchanged attribute still emits a mutation record.
      if (hidden === preferences.statsInPanel) return;
      if (preferences.statsInPanel) strip.dataset.oardshStatsHidden = "";
      else delete strip.dataset.oardshStatsHidden;
    };
    /**
     * The ring: the only dialog trigger drawn as a 14px two-circle gauge. The
     * arcs painted below are circles too and they outlive the panel, so count
     * dsh's own pair only — otherwise the ring stops matching itself after the
     * first hover and never opens again.
     */
    const isContextRing = (element) => {
      if (!element || element.getAttribute("aria-haspopup") !== "dialog") return false;
      const svg = element.querySelector('svg[viewBox="0 0 14 14"]');
      if (!svg) return false;
      return contract("context.ring", svg.querySelectorAll("circle:not([data-oardsh-arc])").length === 2);
    };
    const ringRoot = (button) => {
      let node = button;
      while (node && node !== document.body) {
        if (node.querySelector('[role="dialog"]')) return node;
        node = node.parentElement;
      }
      return button.parentElement;
    };
    const STAT_TINTS = ["#eb6834", "#1baf7a", "#eda100", "#e87ba4", "#e34948"];
    /** The stacked bar: children are the colorful used-context segments. */
    const meterBar = (panel) => {
      for (const el of panel.querySelectorAll("div")) {
        const kids = [...el.children];
        if (kids.length && kids.every((kid) => kid.tagName === "DIV" && /%$/.test(kid.style.width || ""))) return el;
      }
      return null;
    };
    /** Paint the 14px ring with the same tints as the bar, not the gray fill. */
    function paintColorfulRing(root, panel) {
      const svg = root.querySelector('button[aria-haspopup="dialog"] svg[viewBox="0 0 14 14"]');
      if (!svg) return;
      const native = [...svg.querySelectorAll("circle:not([data-oardsh-arc])")];
      const fill = native[1];
      const bar = meterBar(panel);
      const segs = bar ? [...bar.children] : [];
      const signature = segs.map((seg) => `${seg.style.width}:${getComputedStyle(seg).backgroundColor}`).join("|");
      if (svg.dataset.oardshRing === signature) return;
      svg.dataset.oardshRing = signature;
      svg.querySelectorAll("[data-oardsh-arc]").forEach((node) => node.remove());
      if (!fill) return;
      if (segs.length === 0) {
        fill.style.removeProperty("display");
        return;
      }
      fill.style.display = "none";
      const radius = Number(fill.getAttribute("r") || 5.5);
      const ring = 2 * Math.PI * radius;
      let offset = 0;
      for (const seg of segs) {
        const pct = parseFloat(seg.style.width);
        if (!Number.isFinite(pct) || pct <= 0) continue;
        const length = ring * pct / 100;
        const drawn = Math.max(length - 0.8, 0);
        const arc = document.createElementNS("http://www.w3.org/2000/svg", "circle");
        arc.dataset.oardshArc = "";
        arc.setAttribute("cx", fill.getAttribute("cx") || "7");
        arc.setAttribute("cy", fill.getAttribute("cy") || "7");
        arc.setAttribute("r", String(radius));
        arc.setAttribute("fill", "none");
        arc.setAttribute("stroke", getComputedStyle(seg).backgroundColor);
        arc.setAttribute("stroke-width", getComputedStyle(fill).strokeWidth || "2");
        arc.setAttribute("stroke-dasharray", `${drawn} ${Math.max(ring - drawn, 0)}`);
        arc.setAttribute("stroke-dashoffset", String(-offset));
        arc.setAttribute("transform", fill.getAttribute("transform") || "rotate(-90 7 7)");
        svg.appendChild(arc);
        offset += length;
      }
    }
    /** Free remainder sits in dsh's own colorful list, not a second grey block. */
    function ensureFreeRow(panel, t, used, capacity) {
      const dl = panel.querySelector("dl:not(.oardsh-ctx-rows)");
      if (!dl || used === null || capacity === null || capacity <= used) {
        panel.querySelector("[data-oardsh-free]")?.remove();
        return;
      }
      const sample = [...dl.children].find((row) => !row.hasAttribute("data-oardsh-free"));
      let row = dl.querySelector("[data-oardsh-free]");
      if (!row) {
        row = sample ? sample.cloneNode(true) : document.createElement("div");
        row.dataset.oardshFree = "";
        dl.appendChild(row);
      }
      const bar = meterBar(panel);
      const tint = bar ? getComputedStyle(bar).backgroundColor : "var(--dsw-alias-interactive-bg-hover)";
      if (row.style.getPropertyValue("--meter-tint") !== tint) row.style.setProperty("--meter-tint", tint);
      for (const node of row.querySelectorAll("[class]")) {
        const next = node.className.split(/\s+/).filter((name) => !/color(System|Tools|Messages)/i.test(name)).join(" ");
        if (node.className !== next) node.className = next;
      }
      const dt = row.querySelector("dt") || row.appendChild(document.createElement("dt"));
      const dd = row.querySelector("dd") || row.appendChild(document.createElement("dd"));
      let swatch = dt.querySelector("span");
      if (!swatch) {
        swatch = document.createElement("span");
        swatch.setAttribute("aria-hidden", "true");
        dt.prepend(swatch);
      }
      if (!swatch.className.includes("oardsh-ctx-swatch")) swatch.className = `${swatch.className} oardsh-ctx-swatch`.trim();
      if (swatch.style.background !== tint) swatch.style.background = tint;
      const label = t("context.free");
      const labelNode = [...dt.childNodes].find((node) => node.nodeType === Node.TEXT_NODE);
      if (labelNode) {
        if (labelNode.nodeValue !== label) labelNode.nodeValue = label;
      } else {
        dt.appendChild(document.createTextNode(label));
      }
      dd.querySelector("[data-oardsh-share]")?.remove();
      const next = `~${formatTokens(capacity - used)} \u00b7 ${share(capacity - used, capacity)}`;
      if (dd.textContent !== next) dd.textContent = next;
    }
    /**
     * Write the extras into an open panel, comparing before every write: the
     * caller re-runs this from a MutationObserver on the same subtree.
     */
    function decorateContextPanel(root, t) {
      // Not a contract: the panel is absent for the frame between the click and
      // dsh's render, and the observer calls back the moment it lands.
      const panel = root.querySelector('[role="dialog"]');
      if (!panel) return;
      const bucketRows = [...panel.querySelectorAll("dl:not(.oardsh-ctx-rows) > div")].filter((row) => !row.hasAttribute("data-oardsh-free"));
      contract("context.buckets", bucketRows.length > 0);
      const buckets = bucketRows.map((row) => {
        const cell = row.querySelector("dd");
        // Read the original text node, never our own appended share.
        return { cell, tokens: cell ? parseTokens(cell.childNodes[0]?.nodeValue) : null };
      });
      const counted = buckets.filter((bucket) => bucket.tokens !== null);
      const total = counted.reduce((sum, bucket) => sum + bucket.tokens, 0);
      if (total > 0) {
        for (const bucket of counted) {
          let tag = bucket.cell.querySelector("[data-oardsh-share]");
          if (!tag) {
            tag = document.createElement("span");
            tag.dataset.oardshShare = "";
            tag.className = "oardsh-ctx-share";
            bucket.cell.appendChild(tag);
          }
          const reading = ` \u00b7 ${share(bucket.tokens, total)}`;
          if (tag.textContent !== reading) tag.textContent = reading;
        }
      }

      let used = null;
      let capacity = null;
      for (const node of panel.querySelectorAll("span")) {
        const match = /^~?([\d.]+[KM]?)\s*\/\s*([\d.]+[KM]?)$/.exec(node.textContent?.trim() || "");
        if (!match) continue;
        used = parseTokens(match[1]);
        capacity = parseTokens(match[2]);
        break;
      }
      contract("context.figures", used !== null && capacity !== null);
      ensureFreeRow(panel, t, used, capacity);
      contract("context.bar", meterBar(panel));
      paintColorfulRing(root, panel);

      const lines = mirroredStats();
      let extra = panel.querySelector("[data-oardsh-context-extra]");
      if (lines.length === 0) {
        extra?.remove();
        return;
      }
      const signature = JSON.stringify(lines);
      if (extra?.dataset.oardshSignature === signature) return;
      if (!extra) {
        extra = document.createElement("div");
        extra.dataset.oardshContextExtra = "";
        extra.className = "oardsh-ctx-extra";
        panel.appendChild(extra);
      }
      extra.dataset.oardshSignature = signature;
      extra.textContent = "";
      const brand = document.createElement("div");
      brand.className = "oardsh-ctx-brand";
      const glyph = document.createElement("span");
      glyph.className = "oardsh-mark";
      glyph.setAttribute("aria-hidden", "true");
      brand.append(glyph, document.createTextNode("oardsh"));
      extra.append(brand);
      const rows = document.createElement("dl");
      rows.className = "oardsh-ctx-rows";
      extra.append(rows);
      // One row per stat, tinted by the group it came from, so the readings sit
      // in the same right-hand column as dsh's own bucket rows.
      lines.forEach((group, index) => {
        for (const { label, value } of statRows(group)) {
          const row = document.createElement("div");
          row.style.setProperty("--meter-tint", STAT_TINTS[index % STAT_TINTS.length]);
          const term = document.createElement("dt");
          const detail = document.createElement("dd");
          if (value) {
            const swatch = document.createElement("span");
            swatch.className = "oardsh-ctx-swatch";
            swatch.setAttribute("aria-hidden", "true");
            term.append(swatch, document.createTextNode(label));
            detail.textContent = value;
          } else {
            // Nothing numeric to right-align: keep the stat on one full line.
            detail.dataset.wide = "";
            detail.textContent = label;
          }
          row.append(term, detail);
          rows.append(row);
        }
      });
    }

    function Heatmap({ days, t, onTip }) {
      const scroller = useRef(null);
      // A year never fits the column, so open on the newest weeks.
      useLayoutEffect(() => {
        const element = scroller.current;
        if (element) element.scrollLeft = element.scrollWidth;
      }, [days]);
      const max = days.reduce((peak, day) => Math.max(peak, day.totalTokens), 0);
      const pad = days.length ? dayDate(days[0].day).getDay() : 0;
      const cells = [];
      for (let index = 0; index < pad; index += 1) cells.push(h("span", { className: "oardsh-heat-cell", "data-pad": true, key: `pad-${index}` }));
      for (const day of days) {
        const level = day.totalTokens && max ? Math.min(4, Math.max(1, Math.ceil((day.totalTokens / max) * 4))) : 0;
        cells.push(h("span", {
          className: "oardsh-heat-cell", key: day.day, "data-level": level,
          onMouseEnter: (event) => onTip(tipAt(event, dayTip(day, t))),
          onMouseLeave: () => onTip(null),
        }));
      }
      return h("div", { className: "oardsh-card" },
        h(Heading, {
          title: t("heatmap.title"),
          action: h("div", { className: "oardsh-scale" }, t("heatmap.less"),
            [0, 1, 2, 3, 4].map((level) => h("i", { key: level, style: { background: `var(--oardsh-heat-${level})` } })), t("heatmap.more")),
        }),
        h("div", { className: "oardsh-heat-wrap", ref: scroller }, h("div", { className: "oardsh-heatmap" }, cells)));
    }

    function Trend({ days, palette, t, onTip }) {
      const max = days.reduce((peak, day) => Math.max(peak, day.totalTokens), 0);
      // Label every column while they are wide enough, then thin out to five.
      const ticks = new Set(days.length <= 10
        ? days.map((_, index) => index)
        : [0, 0.25, 0.5, 0.75, 1].map((fraction) => Math.round(fraction * (days.length - 1))));
      const models = [];
      for (const day of days) for (const entry of day.models) if (!models.includes(entry.model)) models.push(entry.model);
      return h("div", { className: "oardsh-card" },
        h(Heading, { title: t("trend.title"), action: max ? h("span", { className: "oardsh-peak" }, `${t("trend.peak")} ${compact(max)}`) : undefined }),
        max ? h(React.Fragment, null,
          h("div", { className: "oardsh-plot" },
            h("div", { className: "oardsh-bars" }, days.map((day) => h("div", {
              className: "oardsh-bar", key: day.day,
              onMouseEnter: (event) => onTip(tipAt(event, dayTip(day, t))),
              onMouseLeave: () => onTip(null),
            }, day.models.map((entry) => h("i", {
              key: entry.model, style: { height: `${(entry.totalTokens / max) * 100}%`, background: palette.get(entry.model) },
            })))))),
          h("div", { className: "oardsh-axis" }, days.map((day, index) => h("span", { key: day.day }, ticks.has(index) ? dayLabel(day.day) : ""))),
          models.length > 1 && h("div", { className: "oardsh-legend" }, models.map((model) => h("span", { key: model },
            h("i", { className: "oardsh-dot", style: { background: palette.get(model) } }), model)))
        ) : h("div", { className: "oardsh-empty" }, t("empty")));
    }

    function Models({ models, total, palette, t }) {
      const radius = 54;
      const circumference = 2 * Math.PI * radius;
      let offset = 0;
      const arcs = models.map((entry) => {
        const length = total ? (entry.totalTokens / total) * circumference : 0;
        // A 2px gap keeps neighbouring segments from reading as one arc.
        const drawn = Math.max(length - 2, 0);
        const arc = h("circle", {
          key: entry.model, cx: 80, cy: 80, r: radius, fill: "none", strokeWidth: 20,
          stroke: palette.get(entry.model), strokeDasharray: `${drawn} ${circumference - drawn}`, strokeDashoffset: -offset,
        });
        offset += length;
        return arc;
      });
      return h("div", { className: "oardsh-card" },
        h(Heading, { title: t("models.title") }),
        total ? h("div", { className: "oardsh-donut-row" },
          h("div", { className: "oardsh-donut" },
            h("svg", { width: 160, height: 160, viewBox: "0 0 160 160", role: "presentation" },
              h("g", { transform: "rotate(-90 80 80)" }, arcs)),
            h("div", { className: "oardsh-donut-center" }, h("strong", null, compact(total)), h("span", null, "tokens"))),
          h("ul", { className: "oardsh-list" }, models.map((entry) => h("li", { className: "oardsh-row", key: entry.model },
            h("i", { className: "oardsh-dot", style: { background: palette.get(entry.model) } }),
            h("div", { className: "oardsh-row-main" },
              h("div", { className: "oardsh-row-title", title: entry.model }, entry.model),
              h("div", { className: "oardsh-row-note" }, `${compact(entry.totalTokens)} tokens · ${entry.messages} ${t("turns")}`)),
            h("span", { className: "oardsh-row-value" }, percent((entry.totalTokens / total) * 100)))))
        ) : h("div", { className: "oardsh-empty" }, t("empty")));
    }

    /** oardsh's own row inside dsh's General settings, marked as ours. */
    function StatsPlacementRow({ t }) {
      const [statsInPanel, setStatsInPanel] = useState(preferences.statsInPanel);
      useEffect(() => {
        const sync = () => setStatsInPanel(preferences.statsInPanel);
        preferenceListeners.add(sync);
        sync();
        return () => { preferenceListeners.delete(sync); };
      }, []);
      return h("div", { className: "oardsh-general-row" },
        h("span", { className: "oardsh-mark", "aria-hidden": true, title: "oardsh" }),
        h("div", { className: "oardsh-general-text" },
          h("div", { className: "oardsh-general-title" }, t("statsPlacement.title")),
          h("div", { className: "oardsh-general-help" }, t("statsPlacement.help"))),
        h("div", { className: "oardsh-switch" }, [true, false].map((value) => h("button", {
          type: "button", key: String(value), "data-active": statsInPanel === value || undefined,
          onClick: () => preferences.write?.(value),
        }, t(value ? "statsPlacement.ring" : "statsPlacement.dsh")))));
    }

    function UsageSection({ t }) {
      const [range, setRange] = useState(30);
      const [report, setReport] = useState(null);
      const [tip, setTip] = useState(null);
      const [busy, setBusy] = useState(false);
      const [error, setError] = useState("");
      const native = typeof window.__TAURI_INTERNALS__?.invoke === "function";

      const fail = useCallback((reason) => setError(String(reason).replace(/^Error:\s*/, "")), []);
      // `force` bypasses the command's cache, so Refresh is never a no-op.
      const load = useCallback(async (force = false) => {
        if (!native) return;
        setBusy(true);
        setError("");
        try {
          setReport(await invoke("token_usage", { offsetMinutes: -new Date().getTimezoneOffset(), force }));
        } catch (reason) {
          fail(reason);
        } finally {
          setBusy(false);
        }
      }, [native, fail]);

      useEffect(() => { load(); }, [load]);

      const days = report?.days || [];
      const palette = useMemo(() => {
        const totals = new Map();
        for (const day of days) for (const entry of day.models) totals.set(entry.model, (totals.get(entry.model) || 0) + entry.totalTokens);
        const ordered = [...totals.entries()].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));
        return new Map(ordered.map(([model], index) => [model, colorFor(index)]));
      }, [days]);

      const view = useMemo(() => {
        const slice = days.slice(-range);
        const totals = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, reasoning: 0, total: 0 };
        const sessions = new Set();
        const models = new Map();
        let messages = 0;
        let activeDays = 0;
        for (const day of slice) {
          totals.input += day.inputTokens; totals.output += day.outputTokens;
          totals.cacheRead += day.cacheReadTokens; totals.cacheWrite += day.cacheWriteTokens;
          totals.reasoning += day.reasoningTokens; totals.total += day.totalTokens;
          messages += day.messages;
          for (const id of day.sessions) sessions.add(id);
          if (day.messages) activeDays += 1;
          for (const entry of day.models) {
            const current = models.get(entry.model) || { model: entry.model, totalTokens: 0, messages: 0 };
            current.totalTokens += entry.totalTokens;
            current.messages += entry.messages;
            models.set(entry.model, current);
          }
        }
        return {
          slice, totals, messages, activeDays, sessions: sessions.size,
          models: [...models.values()].sort((a, b) => b.totalTokens - a.totalTokens || a.model.localeCompare(b.model)),
        };
      }, [days, range]);

      // An idle morning must not read as a broken streak, so the walk may
      // start from yesterday.
      const streak = useMemo(() => {
        let index = days.length - 1;
        if (index >= 0 && !days[index].messages) index -= 1;
        let count = 0;
        while (index >= 0 && days[index].messages) { count += 1; index -= 1; }
        return count;
      }, [days]);

      const top = view.models[0];

      return h("section", { className: "oardsh-section" },
        h("div", { className: "oardsh-heading" },
          h("div", { className: "oardsh-brand" }, h("span", { className: "oardsh-mark", "aria-hidden": true }), h("h2", { className: "oardsh-title" }, t("title"))),
          h("p", { className: "oardsh-description" }, t("description"))),
        !native && h("div", { className: "oardsh-error" }, t("unavailable")),
        error && error !== "native-unavailable" && h("div", { className: "oardsh-error" }, `${t("error")}: ${error}`),
        native && h(React.Fragment, null,
          h("div", { className: "oardsh-section-head" },
            h("div", null, h("span", { className: "oardsh-tile-label" }, t("range.label"))),
            h("div", { className: "oardsh-switch" }, RANGES.map((value) => h("button", {
              type: "button", key: value, "data-active": range === value || undefined, onClick: () => setRange(value),
            }, t(`range.${value}`)))),
            h("button", { type: "button", className: "oardsh-button", disabled: busy, onClick: () => load(true) }, busy ? t("loading") : t("refresh"))),
          report ? h(React.Fragment, null,
            h("div", { className: "oardsh-tiles" },
              h(Tile, {
                label: t("stats.tokens"), value: compact(view.totals.total),
                note: `${t("stats.input")} ${compact(view.totals.input)} · ${t("stats.output")} ${compact(view.totals.output)} · ${t("stats.cache")} ${compact(view.totals.cacheRead + view.totals.cacheWrite)}`,
              }),
              h(Tile, { label: t("stats.sessions"), value: view.sessions }),
              h(Tile, { label: t("stats.messages"), value: view.messages }),
              h(Tile, { label: t("stats.activeDays"), value: view.activeDays }),
              h(Tile, { label: t("stats.streak"), value: streak }),
              h(Tile, {
                label: t("stats.topModel"), name: top ? top.model : "—",
                note: top ? `${t("stats.share")} ${percent((top.totalTokens / view.totals.total) * 100)}` : undefined,
              })),
            h(Heatmap, { days, t, onTip: setTip }),
            h(Trend, { days: view.slice, palette, t, onTip: setTip }),
            h(Models, { models: view.models, total: view.totals.total, palette, t })
          ) : h("div", { className: "oardsh-empty" }, t("loading")),
          tip && h("div", { className: "oardsh-tip", style: { left: `${tip.x}px`, top: `${tip.y}px` } },
            h("div", { className: "oardsh-tip-title" }, tip.title),
            h("dl", { className: "oardsh-tip-rows" }, tip.rows.map(([label, value]) => h("div", { key: label },
              h("dt", null, label), h("dd", null, value)))))
        )
      );
    }

    const inject = ["slots", "locale", "connection", "remote", "settingsScope"];
    function apply(ctx) {
      ctx.effect(() => ctx.locale.register(NS, messages), "oardsh: locale dictionaries");
      const t = ctx.locale.bind(NS);
      readLocale = () => intlLocale(ctx.locale.getLocale().active);

      // Durable in dsh's settings document, so it follows the installation:
      // the server binds a fresh port each launch, stranding localStorage.
      const scope = ctx.settingsScope.bind({ namespace: NS });
      const adopt = () => {
        const next = scope.getSnapshot().value?.statsInPanel !== false;
        if (next === preferences.statsInPanel) return;
        preferences.statsInPanel = next;
        applyStatsPlacement();
        notifyPreferences();
      };
      preferences.write = (next) => {
        preferences.statsInPanel = next;
        applyStatsPlacement();
        notifyPreferences();
        scope.set("statsInPanel", next);
      };
      ctx.effect(() => scope.subscribe(adopt), "oardsh: stats placement preference");
      adopt();

      ctx.effect(() => {
        // Cheap: the strip is cached and the callback only writes on a change.
        const observer = new MutationObserver(applyStatsPlacement);
        observer.observe(document.body, { childList: true, subtree: true });
        applyStatsPlacement();
        return () => {
          observer.disconnect();
          const strip = statsStrip();
          if (strip) delete strip.dataset.oardshStatsHidden;
        };
      }, "oardsh: session stats placement");
      ctx.slots.inject("settings.section", () => ctx.slots.register({ name: "settings.section", id: "oardsh-desktop", order: 30, label: () => t("nav") }, () => h(UsageSection, { t })));
      ctx.slots.inject("settings.general.item", () => ctx.slots.register({ name: "settings.general.item", id: "oardsh-stats-placement", order: 60 }, () => h(StatsPlacementRow, { t })));
      ctx.effect(() => {
        let ring = null;
        let root = null;
        let observer = null;
        const decorate = () => {
          if (!root) return;
          observer.disconnect();
          decorateContextPanel(root, t);
          observer.observe(root, { childList: true, subtree: true });
        };
        const release = () => { observer?.disconnect(); observer = null; ring = null; root = null; };
        const open = (button) => {
          if (button.getAttribute("aria-expanded") !== "true") button.click();
          if (ring === button) return;
          release();
          ring = button;
          root = ringRoot(button);
          observer = new MutationObserver(decorate);
          window.requestAnimationFrame(decorate);
        };
        const close = () => {
          // dsh closes the panel itself on Escape or an outside pointerdown,
          // and clicking an already-closed ring would reopen it.
          if (ring?.getAttribute("aria-expanded") === "true") ring.click();
          release();
        };
        const onOver = (event) => {
          const target = event.target instanceof Element ? event.target : null;
          if (!target) return;
          const hovered = target.closest('button[aria-haspopup="dialog"]');
          if (hovered && isContextRing(hovered)) { open(hovered); return; }
          // Cheap exit on the common path: nothing open, nothing to check.
          if (root && !root.contains(target)) close();
        };
        document.addEventListener("mouseover", onOver, true);
        document.addEventListener("mouseleave", close);
        return () => {
          document.removeEventListener("mouseover", onOver, true);
          document.removeEventListener("mouseleave", close);
          release();
        };
      }, "oardsh: hover-open context meter");

      ctx.effect(() => {
        if (typeof window.__TAURI_INTERNALS__?.invoke !== "function") return;
        let lastSignal = "";
        let runningSince = 0;
        let wasRunning = false;
        const notify = async (kind, body = "") => {
          const signal = `${kind}:${body}`;
          if (signal === lastSignal) return;
          lastSignal = signal;
          await invoke("native_web_event", { event: { kind, body, language: locale() } }).catch(() => {});
        };
        const scan = () => {
          const approval = document.querySelector("[data-approval-key],[data-cordis-awaiting]");
          const question = document.querySelector("[data-question-key]");
          const running = Boolean(document.querySelector('[data-state="running"]'));
          if (running && !wasRunning) { runningSince = Date.now(); lastSignal = ""; }
          if (!running && wasRunning && Date.now() - runningSince > 2500 && !approval && !question) notify("completed", document.title);
          wasRunning = running;
          if (approval) notify("approval", approval.textContent?.trim().slice(0, 180) || t("notifications.approval"));
          else if (question) notify("question", question.textContent?.trim().slice(0, 180) || t("notifications.question"));
        };

        // A streaming reply mutates on nearly every frame and each scan walks
        // the whole tree, so coalesce bursts into one scan per frame.
        let queued = 0;
        const schedule = () => {
          if (queued) return;
          queued = window.requestAnimationFrame(() => { queued = 0; scan(); });
        };
        const observer = new MutationObserver(schedule);
        observer.observe(document.documentElement, { childList: true, subtree: true, attributes: true, attributeFilter: ["data-state", "data-approval-key", "data-question-key"] });
        const timer = window.setInterval(scan, 1200);
        scan();
        return () => { observer.disconnect(); window.clearInterval(timer); if (queued) window.cancelAnimationFrame(queued); };
      }, "oardsh: native agent notifications");
    }
    exports.apply = apply;
    exports.inject = inject;
    return module.exports;
  }
});
