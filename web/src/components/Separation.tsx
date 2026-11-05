// SPDX-License-Identifier: AGPL-3.0-or-later

// Separation windows overridden by a category or a clock slot: each one
// either inherited or set here.

import type { SeparationOverrides, Windows } from "../api";
import { type PlainKey, number, t } from "../i18n";
import { type MinuteWindow, WINDOW_FIELDS } from "../lib/rotation";
import { NumberInput } from "./ui";

const LABELS: Record<MinuteWindow, PlainKey> = {
  album_min: "separation.album",
  artist_min: "separation.artist",
  title_min: "separation.title",
  track_min: "separation.track",
};

/** The longest a window can be: a week. */
const MAX_MINUTES = 7 * 24 * 60;

export function SeparationEditor({
  value,
  inherited,
  onChange,
  disabled,
}: {
  value: SeparationOverrides;
  /** What applies where this does not override. */
  inherited: Windows | undefined;
  onChange: (value: SeparationOverrides) => void;
  disabled?: boolean;
}) {
  return (
    <div className="stack">
      {WINDOW_FIELDS.map(([key]) => {
        const own = value[key];
        const overridden = own !== undefined && own !== null;
        const fallback = inherited?.[key];
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
                    next[key] = fallback ?? 0;
                  } else {
                    delete next[key];
                  }
                  onChange(next);
                }}
              />
              {t(LABELS[key])}
            </label>
            {overridden ? (
              <NumberInput
                value={own}
                min={0}
                max={MAX_MINUTES}
                step={1}
                disabled={disabled ?? false}
                suffix={t("common.min")}
                onChange={(minutes) => onChange({ ...value, [key]: Math.round(minutes) })}
              />
            ) : (
              <span className="muted small">
                {fallback === undefined
                  ? t("separation.inherited")
                  : fallback === 0
                    ? t("separation.offInherited")
                    : t("separation.minutesInherited", { minutes: number(fallback) })}
              </span>
            )}
          </div>
        );
      })}
      <p className="muted small" style={{ margin: 0 }}>
        {t("separation.help")}
      </p>
    </div>
  );
}

/** `base` with `overrides` applied. */
export function applyOverrides(base: Windows, overrides: SeparationOverrides | undefined): Windows {
  return {
    album_min: overrides?.album_min ?? base.album_min,
    artist_min: overrides?.artist_min ?? base.artist_min,
    title_min: overrides?.title_min ?? base.title_min,
    track_min: overrides?.track_min ?? base.track_min,
    // Clock slots never override it: only the category's counts.
    dayparts: overrides?.dayparts ?? base.dayparts,
  };
}
