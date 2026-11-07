// SPDX-License-Identifier: AGPL-3.0-or-later

// Charts of the statistics page: listeners minute by minute (a line, broken
// where minutes are missing) and totals per period (bars), one series each,
// so no legend: the card's title names it; and, to compare stations, lines
// one per station with a legend. A tooltip follows the pointer; the tables
// next to them hold the same figures. The geometry is in lib/chart.ts.

import { type PointerEvent, type ReactNode, useLayoutEffect, useRef, useState } from "react";
import { number } from "../i18n";
import {
  HEIGHT,
  PAD,
  type Point,
  areaPath,
  barLayout,
  barPath,
  linePath,
  nearest,
  niceScale,
  splitAtGaps,
  timeTicks,
  tooltipSide,
} from "../lib/chart";

export type { Point } from "../lib/chart";

function useWidth() {
  const ref = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(600);
  useLayoutEffect(() => {
    const element = ref.current;
    if (!element) {
      return;
    }
    setWidth(element.clientWidth || 600);
    const observer = new ResizeObserver(([entry]) => setWidth(entry?.contentRect.width || 600));
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  return { ref, width };
}

function YAxis({ ticks, y, width }: Readonly<{ ticks: number[]; y: (value: number) => number; width: number }>) {
  return (
    <g className="chart-axis">
      {ticks.map((tick) => (
        <g key={tick}>
          <line x1={PAD.left} x2={width - PAD.right} y1={y(tick)} y2={y(tick)} className="chart-grid" />
          <text x={PAD.left - 6} y={y(tick)} dy="0.32em" textAnchor="end">
            {number(tick)}
          </text>
        </g>
      ))}
    </g>
  );
}

function Tooltip({ x, width, children }: Readonly<{ x: number; width: number; children: ReactNode }>) {
  return (
    <div className="chart-tip" style={tooltipSide(x, width)}>
      {children}
    </div>
  );
}

/** Listeners minute by minute between `from` and `to` (Unix ms). */
export function MinuteChart({
  points,
  from,
  to,
  label,
  tickLabel,
  tooltip,
}: Readonly<{
  points: Point[];
  from: number;
  to: number;
  label: string;
  tickLabel: (at: number) => string;
  tooltip: (point: Point) => ReactNode;
}>) {
  const { ref, width } = useWidth();
  const [hover, setHover] = useState<Point | null>(null);
  const { top, ticks } = niceScale(Math.max(0, ...points.map((p) => p.value)));
  const span = Math.max(to - from, 1);
  const x = (at: number) => PAD.left + ((at - from) / span) * (width - PAD.left - PAD.right);
  const y = (value: number) => PAD.top + (1 - value / top) * (HEIGHT - PAD.top - PAD.bottom);

  const move = (event: PointerEvent<SVGSVGElement>) => {
    const box = event.currentTarget.getBoundingClientRect();
    setHover(nearest(points, from + ((event.clientX - box.left - PAD.left) / (width - PAD.left - PAD.right)) * span));
  };

  return (
    <div className="chart" ref={ref}>
      <svg
        width={width}
        height={HEIGHT}
        role="img"
        aria-label={label}
        onPointerMove={move}
        onPointerLeave={() => setHover(null)}
      >
        <YAxis ticks={ticks} y={y} width={width} />
        <g className="chart-axis">
          {timeTicks(from, to, width).map((at) => (
            <text key={at} x={x(at)} y={HEIGHT - 6} textAnchor="middle">
              {tickLabel(at)}
            </text>
          ))}
        </g>
        {splitAtGaps(points).map((segment) => (
          <g key={segment[0]?.at}>
            <path d={areaPath(segment, x, y)} className="chart-area" />
            <path d={linePath(segment, x, y)} className="chart-line" />
            {/* A minute alone between gaps: a dot, a line would not show. */}
            {segment.length === 1 && segment[0] && (
              <circle cx={x(segment[0].at)} cy={y(segment[0].value)} r={2.5} className="chart-dot" />
            )}
          </g>
        ))}
        {hover && (
          <g>
            <line x1={x(hover.at)} x2={x(hover.at)} y1={PAD.top} y2={HEIGHT - PAD.bottom} className="chart-cross" />
            <circle cx={x(hover.at)} cy={y(hover.value)} r={4} className="chart-dot" />
          </g>
        )}
      </svg>
      {hover && (
        <Tooltip x={x(hover.at)} width={width}>
          {tooltip(hover)}
        </Tooltip>
      )}
    </div>
  );
}

export interface Bar {
  key: string;
  /** Under the bar; empty to leave it out where bars are many. */
  label: string;
  value: number;
  tooltip: ReactNode;
}

/** One bar per period. */
export function BarChart({ bars, label }: Readonly<{ bars: Bar[]; label: string }>) {
  const { ref, width } = useWidth();
  const [hover, setHover] = useState<number | null>(null);
  const { top, ticks } = niceScale(Math.max(0, ...bars.map((b) => b.value)));
  const { slot, barWidth, every } = barLayout(bars.length, width);
  const y = (value: number) => PAD.top + (1 - value / top) * (HEIGHT - PAD.top - PAD.bottom);
  const base = y(0);
  const hovered = hover === null ? undefined : bars[hover];

  return (
    <div className="chart" ref={ref}>
      <svg width={width} height={HEIGHT} role="img" aria-label={label} onPointerLeave={() => setHover(null)}>
        <YAxis ticks={ticks} y={y} width={width} />
        {bars.map((bar, i) => {
          const center = PAD.left + slot * i + slot / 2;
          return (
            <g key={bar.key} onPointerEnter={() => setHover(i)}>
              {/* The whole column answers the pointer, not just the bar. */}
              <rect x={PAD.left + slot * i} y={PAD.top} width={slot} height={base - PAD.top} fill="transparent" />
              <path
                d={barPath(center - barWidth / 2, barWidth, base, Math.max(base - y(bar.value), 0))}
                className={hover === i ? "chart-bar hover" : "chart-bar"}
              />
              {bar.label && i % every === 0 && (
                <text x={center} y={HEIGHT - 6} textAnchor="middle" className="chart-axis-text">
                  {bar.label}
                </text>
              )}
            </g>
          );
        })}
      </svg>
      {hovered && hover !== null && (
        <Tooltip x={PAD.left + slot * hover + slot / 2} width={width}>
          {hovered.tooltip}
        </Tooltip>
      )}
    </div>
  );
}

export interface Series {
  key: string;
  label: string;
  /** A CSS color. */
  color: string;
  points: Point[];
}

/**
 * Several lines over the same axis, one color each, with a legend: `at` is a
 * time (Unix ms) or an index (days, hours); lines break where a point is
 * missing (`step` apart). The tooltip gives every line's value at the point.
 */
export function LinesChart({
  series,
  from,
  to,
  step,
  ticks,
  label,
  tickLabel,
  tooltip,
}: Readonly<{
  series: Series[];
  from: number;
  to: number;
  /** How far apart two points of an unbroken line are. */
  step: number;
  ticks: number[];
  label: string;
  tickLabel: (at: number) => string;
  tooltip: (at: number, values: { series: Series; value: number | undefined }[]) => ReactNode;
}>) {
  const { ref, width } = useWidth();
  const [hover, setHover] = useState<number | null>(null);
  const all = series.flatMap((s) => s.points);
  const { top, ticks: yTicks } = niceScale(Math.max(0, ...all.map((p) => p.value)));
  const span = Math.max(to - from, 1);
  const x = (at: number) => PAD.left + ((at - from) / span) * (width - PAD.left - PAD.right);
  const y = (value: number) => PAD.top + (1 - value / top) * (HEIGHT - PAD.top - PAD.bottom);

  const move = (event: PointerEvent<SVGSVGElement>) => {
    const box = event.currentTarget.getBoundingClientRect();
    const at = from + ((event.clientX - box.left - PAD.left) / (width - PAD.left - PAD.right)) * span;
    setHover(nearest(all, at, Math.max(step * 5, span / 50))?.at ?? null);
  };

  return (
    <div className="stack" style={{ gap: "0.4rem" }}>
      <div className="chart" ref={ref}>
        <svg width={width} height={HEIGHT} role="img" aria-label={label} onPointerMove={move} onPointerLeave={() => setHover(null)}>
          <YAxis ticks={yTicks} y={y} width={width} />
          <g className="chart-axis">
            {ticks.map((at) => (
              <text key={at} x={x(at)} y={HEIGHT - 6} textAnchor="middle">
                {tickLabel(at)}
              </text>
            ))}
          </g>
          {series.map((s) =>
            splitAtGaps(s.points, step).map((segment) => (
              <g key={`${s.key}-${segment[0]?.at}`}>
                <path d={linePath(segment, x, y)} className="chart-line" style={{ stroke: s.color }} />
                {segment.length === 1 && segment[0] && (
                  <circle cx={x(segment[0].at)} cy={y(segment[0].value)} r={2.5} className="chart-dot" style={{ fill: s.color }} />
                )}
              </g>
            )),
          )}
          {hover !== null && (
            <g>
              <line x1={x(hover)} x2={x(hover)} y1={PAD.top} y2={HEIGHT - PAD.bottom} className="chart-cross" />
              {series.map((s) => {
                const point = s.points.find((p) => p.at === hover);
                return point ? <circle key={s.key} cx={x(hover)} cy={y(point.value)} r={4} className="chart-dot" style={{ fill: s.color }} /> : null;
              })}
            </g>
          )}
        </svg>
        {hover !== null && (
          <Tooltip x={x(hover)} width={width}>
            {tooltip(
              hover,
              series.map((s) => ({ series: s, value: s.points.find((p) => p.at === hover)?.value })),
            )}
          </Tooltip>
        )}
      </div>
      <Legend series={series} />
    </div>
  );
}

/** The stations of a chart and their colors. */
export function Legend({ series }: Readonly<{ series: Pick<Series, "key" | "label" | "color">[] }>) {
  return (
    <ul className="chart-legend">
      {series.map((s) => (
        <li key={s.key}>
          <span className="chart-swatch" style={{ background: s.color }} aria-hidden="true" />
          {s.label}
        </li>
      ))}
    </ul>
  );
}
