// SPDX-License-Identifier: AGPL-3.0-or-later

// The draw depth a category or a clock slot sets: its start and its
// maximum, each inherited or set here (docs/rotation.md#draw-depth).

import type { DrawDepth, DrawLog, SeparationOverrides } from "../api";
import { type PlainKey, number, t } from "../i18n";
import { NumberInput } from "./ui";

type DrawKey = "draw_start_pct" | "draw_max_pct";

const FIELDS: [DrawKey, keyof DrawDepth, PlainKey][] = [
  ["draw_start_pct", "start_pct", "draw.start"],
  ["draw_max_pct", "max_pct", "draw.max"],
];

/** The station's depth if the settings are not loaded yet. */
export const DEFAULT_DRAW: DrawDepth = { start_pct: 20, max_pct: 50 };

/** `base` with what `overrides` sets; a start above the maximum at the maximum. */
export function applyDraw(base: DrawDepth, overrides: SeparationOverrides | undefined): DrawDepth {
  const max_pct = overrides?.draw_max_pct ?? base.max_pct;
  const start_pct = Math.min(overrides?.draw_start_pct ?? base.start_pct, max_pct);
  return { start_pct, max_pct };
}

/** A pick's draw: `30 %, 30 tracks, 4 held back by the artist`. */
export function describeDrawLog(draw: DrawLog): string {
  const blocked = draw.blocked;
  const held = [
    blocked.track && t("draw.heldTrack", { count: blocked.track }),
    blocked.title && t("draw.heldTitle", { count: blocked.title }),
    blocked.artist && t("draw.heldArtist", { count: blocked.artist }),
    blocked.album && t("draw.heldAlbum", { count: blocked.album }),
    blocked.daypart && t("draw.heldDaypart", { count: blocked.daypart }),
    blocked.daypart_days && t("draw.heldDaypartDays", { count: blocked.daypart_days }),
    blocked.across && t("draw.heldAcross", { count: blocked.across }),
  ].filter(Boolean);
  return [
    t("draw.pct", { pct: number(draw.pct) }),
    t("common.tracks", { count: draw.tracks }),
    ...held,
  ].join(", ");
}

export function DrawDepthEditor({
  value,
  inherited,
  onChange,
  disabled,
}: {
  value: SeparationOverrides;
  /** What applies where this does not set its own. */
  inherited: DrawDepth;
  onChange: (value: SeparationOverrides) => void;
  disabled?: boolean;
}) {
  const start = value.draw_start_pct ?? inherited.start_pct;
  const max = value.draw_max_pct ?? inherited.max_pct;
  return (
    <div className="stack">
      {FIELDS.map(([key, from, label]) => {
        const own = value[key];
        const overridden = own !== undefined && own !== null;
        return (
          <div key={key} className="row between">
            <label className="check">
              <input
                type="checkbox"
                checked={overridden}
                disabled={disabled}
                onChange={(event) => {
                  const next = { ...value };
                  if (event.target.checked) {
                    next[key] = inherited[from];
                  } else {
                    delete next[key];
                  }
                  onChange(next);
                }}
              />
              {t(label)}
            </label>
            {overridden ? (
              <NumberInput
                value={own}
                min={key === "draw_max_pct" ? Math.max(1, value.draw_start_pct ?? 1) : 1}
                max={key === "draw_start_pct" ? (value.draw_max_pct ?? 100) : 100}
                step={1}
                disabled={disabled ?? false}
                suffix="%"
                label={t(label)}
                onChange={(pct) => onChange({ ...value, [key]: Math.round(pct) })}
              />
            ) : (
              <span className="muted small">{t("draw.inherited", { pct: number(inherited[from]) })}</span>
            )}
          </div>
        );
      })}
      <p className="muted small" style={{ margin: 0 }}>
        {t("draw.help", { start: number(Math.min(start, max)), max: number(max) })}
      </p>
    </div>
  );
}
