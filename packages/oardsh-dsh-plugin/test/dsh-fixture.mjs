/**
 * dsh's own markup, transcribed from `@deepseek-ai/dsh-client-ui-conversation`
 * (ContextMeter and StatsLine). Transcribed rather than approximated on purpose:
 * an invented fixture would keep passing after dsh moves, which is the one
 * outcome worse than no test. `upstream.test.mjs` checks it against the real
 * package.
 */

/// dsh's CSS-module class names are hashed per build, so the plugin must never
/// match on them. They are reproduced here precisely because they are unstable:
/// a test that leans on one is testing the wrong thing.
const CSS = {
  root: "JObwrW_root",
  trigger: "JObwrW_trigger",
  track: "JObwrW_track",
  fill: "JObwrW_fill",
  panel: "JObwrW_panel",
  header: "JObwrW_header",
  figures: "JObwrW_figures",
  bar: "JObwrW_bar",
  segment: "JObwrW_segment",
  rows: "JObwrW_rows",
  row: "JObwrW_row",
  swatch: "JObwrW_swatch",
};

export const BUCKETS = [
  { label: "System prompt", tokens: "~12.3K", tint: "JObwrW_colorSystem" },
  { label: "Tools", tokens: "~8.1K", tint: "JObwrW_colorTools" },
  { label: "Messages", tokens: "~37.2K", tint: "JObwrW_colorMessages" },
];

/// One group per `|`-separated run, exactly as StatsLine joins them.
export const STATS_EN = [
  "2 turns · 5 steps",
  "LLM 35m38s · Tool call 20m38s",
  "TTFT avg 9.9s · 97 tok/s",
  "Cache hit 99%",
  "Input 13.6M tok · Output 0.1M tok",
];
export const STATS_ZH = [
  "2 轮 · 5 步",
  "LLM 35m38s · 工具调用 20m38s",
  "首 token 平均 9.9s · 97 tok/s",
  "缓存命中 99%",
  "输入 13.6M tok · 输出 0.1M tok",
];

/**
 * The context meter, wired the way React wires it: the trigger toggles
 * `aria-expanded` and the panel appears and disappears with it, so a plugin
 * that leaks state across an open/close cycle fails here the way it fails live.
 */
export function mountContextMeter(document, { percent = 45, used = "~57.6K", total = "128K" } = {}) {
  const root = document.createElement("span");
  root.className = CSS.root;
  root.innerHTML = `
    <button type="button" class="${CSS.trigger}" aria-label="${percent}% of context used"
            aria-haspopup="dialog" aria-expanded="false">
      <svg viewBox="0 0 14 14" width="14" height="14" aria-hidden="true">
        <circle class="${CSS.track}" cx="7" cy="7" r="5.5"></circle>
        <circle class="${CSS.fill}" cx="7" cy="7" r="5.5" stroke-dasharray="15.5 34.5"
                transform="rotate(-90 7 7)"></circle>
      </svg>
    </button>`;
  const trigger = root.querySelector("button");

  const panel = () => {
    const node = document.createElement("div");
    node.className = CSS.panel;
    node.setAttribute("role", "dialog");
    node.setAttribute("aria-label", "of context used");
    const segments = BUCKETS.map(
      (bucket, index) =>
        `<div class="${CSS.segment} ${bucket.tint}" style="width: ${(index + 1) * 5}%"></div>`,
    ).join("");
    const rows = BUCKETS.map(
      (bucket) =>
        `<div class="${CSS.row}"><dt><span class="${CSS.swatch} ${bucket.tint}" aria-hidden="true"></span>${bucket.label}</dt><dd>${bucket.tokens}</dd></div>`,
    ).join("");
    node.innerHTML = `
      <div class="${CSS.header}">
        <span class="JObwrW_headline"></span>
        <span class="JObwrW_percent">${percent}%</span>
        <span class="JObwrW_headline">of context used</span>
        <span class="${CSS.figures}">${used} / ${total}</span>
      </div>
      <div class="${CSS.bar}">${segments}</div>
      <dl class="${CSS.rows}">${rows}</dl>`;
    return node;
  };

  trigger.addEventListener("click", () => {
    const open = trigger.getAttribute("aria-expanded") === "true";
    trigger.setAttribute("aria-expanded", open ? "false" : "true");
    if (open) root.querySelector('[role="dialog"]')?.remove();
    else root.appendChild(panel());
  });

  return { root, trigger, svg: root.querySelector("svg") };
}

/** dsh's stats line: groups in spans, joined by an aria-hidden `|`. */
export function mountStatsLine(document, groups = STATS_EN) {
  const line = document.createElement("div");
  for (const [index, group] of groups.entries()) {
    if (index > 0) {
      const separator = document.createElement("span");
      separator.setAttribute("aria-hidden", "true");
      separator.textContent = "|";
      line.append(separator, document.createTextNode(" "));
    }
    const span = document.createElement("span");
    span.textContent = group;
    line.append(span);
  }
  return line;
}
