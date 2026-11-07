// SPDX-License-Identifier: AGPL-3.0-or-later

// Geometry of the statistics charts (components/ListenerChart.tsx): scales,
// lines broken at missing minutes, bars rounded at their data end.

export const HEIGHT = 180;
export const PAD = { top: 12, right: 12, bottom: 22, left: 36 } as const;

const MINUTE = 60_000;
const HOUR = 3_600_000;

export interface Point {
  at: number;
  value: number;
}

/** A round top for the y axis, and 3 or 4 steps up to it. */
export function niceScale(max: number): { top: number; ticks: number[] } {
  if (max <= 0) {
    return { top: 1, ticks: [0, 1] };
  }
  const raw = max / 4;
  const magnitude = 10 ** Math.floor(Math.log10(raw));
  const step = [1, 2, 5, 10].map((m) => m * magnitude).find((s) => s >= raw) ?? raw;
  const roundStep = Math.max(step, 1);
  const top = Math.ceil(max / roundStep) * roundStep;
  const ticks = [];
  for (let tick = 0; tick <= top + roundStep / 2; tick += roundStep) {
    ticks.push(tick);
  }
  return { top, ticks };
}

/** Points split wherever one is missing: a minute, or `step` apart. */
export function splitAtGaps(points: readonly Point[], step = MINUTE): Point[][] {
  const segments: Point[][] = [];
  for (const point of points) {
    const segment = segments.at(-1);
    const previous = segment?.at(-1);
    if (segment && previous && point.at - previous.at <= step) {
      segment.push(point);
    } else {
      segments.push([point]);
    }
  }
  return segments;
}

type Scale = (value: number) => number;

/** The SVG path of a line through the points. */
export function linePath(segment: readonly Point[], x: Scale, y: Scale): string {
  return segment.map((p, i) => `${i === 0 ? "M" : "L"}${x(p.at).toFixed(1)},${y(p.value).toFixed(1)}`).join("");
}

/** The SVG path of the area under the line, down to 0. */
export function areaPath(segment: readonly Point[], x: Scale, y: Scale): string {
  const first = segment.at(0);
  const last = segment.at(-1);
  if (!first || !last) {
    return "";
  }
  return `${linePath(segment, x, y)}L${x(last.at).toFixed(1)},${y(0)}L${x(first.at).toFixed(1)},${y(0)}Z`;
}

/** Ticks every 3 hours from `from` (every 6 on narrow charts), the end left out: its label would leave the chart. */
export function timeTicks(from: number, to: number, width: number): number[] {
  const step = (width < 480 ? 6 : 3) * HOUR;
  const ticks: number[] = [];
  for (let at = from; at < to - step / 2; at += step) {
    ticks.push(at);
  }
  return ticks;
}

/** The point nearest to `at`, if one is within `within` ms. */
export function nearest(points: readonly Point[], at: number, within = 5 * MINUTE): Point | null {
  let best: Point | null = null;
  for (const point of points) {
    if (!best || Math.abs(point.at - at) < Math.abs(best.at - at)) {
      best = point;
    }
  }
  return best && Math.abs(best.at - at) <= within ? best : null;
}

/** Where bars go: a slot each, a 2px gap between them, thin when they are few, and a label every few bars. */
export function barLayout(count: number, width: number): { slot: number; barWidth: number; every: number } {
  const inner = width - PAD.left - PAD.right;
  const slot = inner / Math.max(count, 1);
  return {
    slot,
    barWidth: Math.max(1, Math.min(slot - 2, 48)),
    every: Math.max(1, Math.ceil(count / Math.max(1, Math.floor(inner / 56)))),
  };
}

/** A bar rounded at its data end and square on the baseline; empty when it has no height. */
export function barPath(left: number, width: number, base: number, height: number): string {
  if (height <= 0) {
    return "";
  }
  const radius = Math.min(4, width / 2, height);
  const top = base - height;
  return (
    `M${left},${base}V${top + radius}Q${left},${top} ${left + radius},${top}H${left + width - radius}` +
    `Q${left + width},${top} ${left + width},${top + radius}V${base}Z`
  );
}

/** Where a tooltip goes: right of the pointer, or left of it near the right edge. */
export function tooltipSide(x: number, width: number): { left: number } | { right: number } {
  return x > width * 0.65 ? { right: width - x + 8 } : { left: x + 8 };
}
