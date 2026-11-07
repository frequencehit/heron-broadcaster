// SPDX-License-Identifier: AGPL-3.0-or-later

// The gains of two tracks around their transition, as the engine plans it.

import { useEffect, useRef, useState } from "react";
import type { TransitionPreview } from "../api";
import { incomingGain, outgoingGain } from "../lib/fade";
import { t } from "../i18n";
import { duration } from "../lib/format";
import type { WaveformData } from "./Waveform";

/** The height of the track page's waveform (11rem, see `.waveform canvas`), in pixels: the graph's where it is shown at a fixed height. */
export const WAVEFORM_HEIGHT = 176;

/** The drawing's size when it scales with its width. */
const DEFAULT_W = 720;
const DEFAULT_H = 200;
const PAD = { top: 12, right: 12, bottom: 26, left: 40 };
const STEPS = 240;

function peakAt(data: WaveformData | null | undefined, ms: number): number {
  if (!data || ms < 0) {
    return 0;
  }
  return (data.peaks[Math.floor(ms / data.intervalMs)] ?? 0) / 255;
}

/**
 * A window around the transition, on the outgoing track's timeline: from a
 * few seconds before it starts to a few seconds after it ends. The graph
 * shows it, and the preview player plays it.
 */
export function transitionWindow(preview: TransitionPreview): { from: number; to: number } {
  const out = preview.outgoing;
  const fadeStart = preview.fade_out?.start_ms ?? preview.next_start_ms;
  const from = Math.max(out.cue_in_ms, Math.min(fadeStart, preview.next_start_ms) - 4_000);
  const to = Math.max(out.cue_out_ms, preview.next_start_ms + (preview.fade_in?.length_ms ?? 0)) + 4_000;
  return { from, to };
}

export function TransitionGraph({
  preview,
  outgoingWave,
  incomingWave,
  playhead,
  height,
}: {
  preview: TransitionPreview;
  outgoingWave?: WaveformData | null;
  incomingWave?: WaveformData | null;
  /** Where the preview player is, on the outgoing track's timeline. */
  playhead?: number | null;
  /** A fixed height in pixels, its width the container's: else it scales with its width. */
  height?: number;
}) {
  const figure = useRef<HTMLElement>(null);
  const [measured, setMeasured] = useState<number | null>(null);
  useEffect(() => {
    const element = figure.current;
    if (height === undefined || !element || typeof ResizeObserver === "undefined") {
      return;
    }
    setMeasured(element.clientWidth || null);
    const observer = new ResizeObserver(() => setMeasured(element.clientWidth || null));
    observer.observe(element);
    return () => observer.disconnect();
  }, [height]);
  const W = height !== undefined && measured ? measured : DEFAULT_W;
  const H = height ?? DEFAULT_H;
  const out = preview.outgoing;
  const into = preview.incoming;
  const { from, to } = transitionWindow(preview);
  const span = Math.max(1, to - from);
  const plotW = W - PAD.left - PAD.right;
  const plotH = H - PAD.top - PAD.bottom;
  const x = (ms: number) => PAD.left + ((ms - from) / span) * plotW;
  const y = (gain: number) => PAD.top + (1 - gain) * plotH;
  // The incoming track's own time at `ms` on the outgoing timeline.
  const incomingTime = (ms: number) => ms - preview.next_start_ms + into.cue_in_ms;

  const points = (gain: (ms: number) => number) =>
    Array.from({ length: STEPS + 1 }, (_, i) => {
      const ms = from + (i / STEPS) * span;
      return `${x(ms).toFixed(1)},${y(gain(ms)).toFixed(1)}`;
    }).join(" ");
  const outGain = (ms: number) => (ms < out.cue_in_ms ? 0 : outgoingGain(preview.fade_out, out.cue_out_ms, ms));
  const inGain = (ms: number) =>
    ms < preview.next_start_ms ? 0 : incomingGain(preview.fade_in, into.cue_in_ms, incomingTime(ms));

  // Waveforms scaled by their gain, as heard.
  const wave = (data: WaveformData | null | undefined, time: (ms: number) => number, gain: (ms: number) => number) => {
    if (!data) {
      return "";
    }
    const top: string[] = [];
    const bottom: string[] = [];
    for (let i = 0; i <= STEPS; i += 1) {
      const ms = from + (i / STEPS) * span;
      const level = peakAt(data, time(ms)) * gain(ms) * 0.5;
      top.push(`${x(ms).toFixed(1)},${(PAD.top + plotH / 2 - level * plotH).toFixed(1)}`);
      bottom.unshift(`${x(ms).toFixed(1)},${(PAD.top + plotH / 2 + level * plotH).toFixed(1)}`);
    }
    return [...top, ...bottom].join(" ");
  };

  const ticks = [0, 0.5, 1];
  const timeTicks = [from, preview.next_start_ms, out.cue_out_ms, to].filter(
    (ms, i, all) => all.findIndex((other) => Math.abs(other - ms) < span / 12) === i,
  );

  return (
    <figure ref={figure} style={{ margin: 0 }}>
      <svg
        className="graph"
        viewBox={`0 0 ${W} ${H}`}
        style={height !== undefined ? { height } : undefined}
        role="img"
        aria-label={t("transition.graphLabel", { time: duration(out.cue_out_ms - preview.next_start_ms, true) })}
      >
        {ticks.map((gain) => (
          <g key={gain}>
            <line x1={PAD.left} x2={W - PAD.right} y1={y(gain)} y2={y(gain)} stroke="var(--border)" />
            <text x={PAD.left - 6} y={y(gain) + 4} textAnchor="end" fontSize="11" fill="var(--muted)">
              {Math.round(gain * 100)}%
            </text>
          </g>
        ))}
        <polygon points={wave(outgoingWave, (ms) => ms, outGain)} fill="var(--deck-a)" opacity="0.18" />
        <polygon points={wave(incomingWave, incomingTime, inGain)} fill="var(--deck-b)" opacity="0.18" />
        {preview.overlap_ms > 0 && (
          <rect
            x={x(preview.next_start_ms)}
            y={PAD.top}
            width={Math.max(1, x(out.cue_out_ms) - x(preview.next_start_ms))}
            height={plotH}
            fill="var(--accent)"
            opacity="0.08"
          />
        )}
        <polyline points={points(outGain)} fill="none" stroke="var(--deck-a)" strokeWidth="2.5" />
        <polyline points={points(inGain)} fill="none" stroke="var(--deck-b)" strokeWidth="2.5" />
        <line
          x1={x(preview.next_start_ms)}
          x2={x(preview.next_start_ms)}
          y1={PAD.top}
          y2={PAD.top + plotH}
          stroke="var(--accent)"
          strokeDasharray="4 3"
        />
        <line
          x1={x(out.cue_out_ms)}
          x2={x(out.cue_out_ms)}
          y1={PAD.top}
          y2={PAD.top + plotH}
          stroke="var(--bad)"
          strokeDasharray="4 3"
        />
        {playhead !== null && playhead !== undefined && playhead >= from && playhead <= to && (
          <line
            x1={x(playhead)}
            x2={x(playhead)}
            y1={PAD.top}
            y2={PAD.top + plotH}
            stroke="var(--fg)"
            strokeWidth="1.5"
          />
        )}
        {timeTicks.map((ms, index) => (
          <text
            key={ms}
            x={x(ms)}
            y={H - 8}
            textAnchor={index === 0 ? "start" : index === timeTicks.length - 1 ? "end" : "middle"}
            fontSize="11"
            fill="var(--muted)"
          >
            {duration(ms, true)}
          </text>
        ))}
      </svg>
      <figcaption className="legend muted">
        <span>
          <span className="swatch" style={{ background: "var(--deck-a)" }} /> {t("transition.outgoing")}
        </span>
        <span>
          <span className="swatch" style={{ background: "var(--deck-b)" }} /> {t("transition.incoming")}
        </span>
        <span>
          <span className="swatch" style={{ background: "var(--accent)" }} /> {t("transition.nextStarts")}
        </span>
        <span>
          <span className="swatch" style={{ background: "var(--bad)" }} /> {t("transition.cueOut")}
        </span>
        <span>
          {preview.overlap_ms > 0
            ? t("transition.overlap", { time: duration(preview.overlap_ms, true) })
            : t("transition.noOverlap")}
        </span>
      </figcaption>
    </figure>
  );
}
