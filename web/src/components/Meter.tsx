// SPDX-License-Identifier: AGPL-3.0-or-later

// Level meters: RMS as the bar, peak as a tick, from -60 dBFS to 0.

import type { ChannelLevels } from "../api";
import { number, t } from "../i18n";
import { REDUCTION_RANGE_DB, formatDb, reductionScale } from "../lib/agc";

const FLOOR_DB = -60;

/** Position of a level on the meter, 0 to 1. */
export function meterScale(db: number): number {
  if (!Number.isFinite(db)) {
    return 0;
  }
  return Math.min(1, Math.max(0, (db - FLOOR_DB) / -FLOOR_DB));
}

function Bar({ rms, peak, label }: Readonly<{ rms: number; peak: number; label: string }>) {
  return (
    <div className="meter-bar" title={t("meter.peak", { channel: label, level: number(peak, { maximumFractionDigits: 1, minimumFractionDigits: 1 }) })}>
      <div className="rms" style={{ height: `${meterScale(rms) * 100}%` }} />
      <div className="peak" style={{ bottom: `calc(${meterScale(peak) * 100}% - 2px)` }} />
    </div>
  );
}

/** A vertical stereo meter. */
export function StereoMeter({ levels, label }: Readonly<{ levels: ChannelLevels | null; label: string }>) {
  const silent = { peak_left: -200, peak_right: -200, rms_left: -200, rms_right: -200 };
  const l = levels ?? silent;
  const loudest = Math.max(l.peak_left, l.peak_right);
  return (
    <div className="meter" role="meter" aria-label={t("meter.level", { name: label })} aria-valuemin={FLOOR_DB}
      aria-valuemax={0} aria-valuenow={Math.max(FLOOR_DB, Math.round(loudest))}>
      <div className="meter-bars">
        <Bar rms={l.rms_left} peak={l.peak_left} label={t("meter.left")} />
        <Bar rms={l.rms_right} peak={l.peak_right} label={t("meter.right")} />
      </div>
      <span>{label}</span>
      <span className="num">{loudest > FLOOR_DB ? number(Math.round(loudest)) : "–∞"}</span>
    </div>
  );
}

/** A thin horizontal meter (the louder channel's RMS). */
export function LineMeter({ levels, label }: Readonly<{ levels: ChannelLevels | null; label: string }>) {
  const rms = levels ? Math.max(levels.rms_left, levels.rms_right) : -200;
  const width = meterScale(rms) * 100;
  return (
    <div className="hmeter" role="meter" aria-label={t("meter.level", { name: label })} aria-valuemin={FLOOR_DB}
      aria-valuemax={0} aria-valuenow={Math.max(FLOOR_DB, Math.round(rms))}
      style={{ ["--meter-width" as string]: `${width > 0 ? 10000 / width : 100}%` }}>
      <div style={{ width: `${width}%` }} />
    </div>
  );
}

/**
 * A gain reduction (dB, 0 or less), filling from the left as it deepens,
 * to 24 dB. Empty and dashed while there is none to show.
 */
export function ReductionMeter({ db, label }: Readonly<{ db: number | null | undefined; label: string }>) {
  const known = db !== null && db !== undefined && Number.isFinite(db);
  return (
    <div className={known ? "gr-meter" : "gr-meter idle"}>
      {/* The native meter for assistive technologies; the bar below is drawn. */}
      <meter className="visually-hidden" aria-label={t("meter.reduction", { name: label })}
        min={-REDUCTION_RANGE_DB} max={0} value={known ? Math.round(db) : 0} />
      <div className="gr-track" aria-hidden="true">
        <div style={{ width: `${reductionScale(db) * 100}%` }} />
      </div>
      <span className="num">{known ? formatDb(db) : "–"}</span>
    </div>
  );
}
