// Single-series bar charts: one hue (magnitude), thin bars with a 4px rounded data end,
// selective value labels, recessive axes, per-bar hover. No legend: the title names the series.

import { fmt, h, hideTip, s, showTip } from "./ui";

export interface Datum {
  label: string;
  value: number;
  /** De-emphasised bar (e.g. "unreachable", "one-off pages"). */
  muted?: boolean;
  tip?: [string, string][];
}

/** Path for a bar with rounded corners at the data end only. */
function barPath(x: number, y: number, w: number, hgt: number, r: number, horizontal: boolean): string {
  r = Math.min(r, horizontal ? hgt / 2 : w / 2, horizontal ? w : hgt);
  if (horizontal) {
    // baseline on the left, data end on the right
    return `M${x},${y}h${w - r}a${r},${r} 0 0 1 ${r},${r}v${hgt - 2 * r}a${r},${r} 0 0 1 ${-r},${r}h${-(w - r)}z`;
  }
  return `M${x},${y + hgt}v${-(hgt - r)}a${r},${r} 0 0 1 ${r},${-r}h${w - 2 * r}a${r},${r} 0 0 1 ${r},${r}v${hgt - r}z`;
}

function niceMax(v: number): number {
  if (v <= 0) return 1;
  const p = 10 ** Math.floor(Math.log10(v));
  return [1, 2, 2.5, 5, 10].map((m) => m * p).find((m) => m >= v) ?? v;
}

export function columnChart(data: Datum[], opts: { height?: number; xLabel?: string } = {}): HTMLElement {
  const W = 640, H = opts.height ?? 220, top = 24, bottom = opts.xLabel ? 44 : 28, left = 44, right = 8;
  const max = niceMax(Math.max(...data.map((d) => d.value)));
  const plotH = H - top - bottom, plotW = W - left - right;
  const band = plotW / Math.max(1, data.length);
  const barW = Math.min(24, band * 0.6);
  const y = (v: number) => top + plotH - (v / max) * plotH;

  const svg = s("svg", { viewBox: `0 0 ${W} ${H}`, role: "img" });
  for (let i = 0; i <= 4; i++) {
    const v = (max / 4) * i, yy = y(v);
    svg.append(s("line", { x1: left, x2: W - right, y1: yy, y2: yy, class: i === 0 ? "baseline" : "gridline" }));
    const t = s("text", { x: left - 8, y: yy + 4, "text-anchor": "end" });
    t.textContent = fmt.compact(v);
    svg.append(t);
  }
  data.forEach((d, i) => {
    const cx = left + band * i + band / 2;
    const hgt = Math.max(0, y(0) - y(d.value));
    if (hgt > 0) {
      svg.append(s("path", { d: barPath(cx - barW / 2, y(d.value), barW, hgt, 4, false), class: d.muted ? "bar muted-bar" : "bar" }));
    }
    const val = s("text", { x: cx, y: y(d.value) - 6, "text-anchor": "middle", class: "value" });
    val.textContent = fmt.compact(d.value);
    svg.append(val);
    const lab = s("text", { x: cx, y: H - bottom + 18, "text-anchor": "middle" });
    lab.textContent = d.label;
    svg.append(lab);
    // Hit target spans the whole band, larger than the bar.
    const hit = s("rect", { x: cx - band / 2, y: top, width: band, height: plotH, fill: "transparent", tabindex: 0 });
    const show = (e: { clientX: number; clientY: number }) =>
      showTip(e.clientX, e.clientY, d.tip ?? [["pages", fmt.int(d.value)]], d.label);
    hit.addEventListener("pointermove", show);
    hit.addEventListener("pointerleave", hideTip);
    hit.addEventListener("focus", () => {
      const r = hit.getBoundingClientRect();
      show({ clientX: r.right, clientY: r.top });
    });
    hit.addEventListener("blur", hideTip);
    svg.append(hit);
  });
  if (opts.xLabel) {
    const t = s("text", { x: left + plotW / 2, y: H - 6, "text-anchor": "middle" });
    t.textContent = opts.xLabel;
    svg.append(t);
  }
  return h("div", { class: "chart" }, svg);
}

export function barChart(data: Datum[], opts: { labelWidth?: number } = {}): HTMLElement {
  const rowH = 30, barH = 16, labelW = opts.labelWidth ?? 260, right = 56, W = 640;
  const H = data.length * rowH + 8;
  const max = Math.max(1, ...data.map((d) => d.value));
  const plotW = W - labelW - right;
  const svg = s("svg", { viewBox: `0 0 ${W} ${H}`, role: "img" });
  svg.append(s("line", { x1: labelW, x2: labelW, y1: 0, y2: H, class: "baseline" }));
  data.forEach((d, i) => {
    const y = i * rowH + (rowH - barH) / 2;
    const w = (d.value / max) * plotW;
    const lab = s("text", { x: labelW - 10, y: y + barH / 2 + 4, "text-anchor": "end" });
    lab.textContent = d.label.length > 40 ? `${d.label.slice(0, 39)}…` : d.label;
    svg.append(lab);
    if (w > 0) svg.append(s("path", { d: barPath(labelW, y, w, barH, 4, true), class: d.muted ? "bar muted-bar" : "bar" }));
    const val = s("text", { x: labelW + w + 6, y: y + barH / 2 + 4, class: "value" });
    val.textContent = fmt.int(d.value);
    svg.append(val);
    const hit = s("rect", { x: 0, y: i * rowH, width: W, height: rowH, fill: "transparent", tabindex: 0 });
    hit.addEventListener("pointermove", (e) => showTip(e.clientX, e.clientY, d.tip ?? [["pages", fmt.int(d.value)]], d.label));
    hit.addEventListener("pointerleave", hideTip);
    svg.append(hit);
  });
  return h("div", { class: "chart" }, svg);
}
