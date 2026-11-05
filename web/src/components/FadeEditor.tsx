// SPDX-License-Identifier: AGPL-3.0-or-later

import type { Fade, FadeCurve } from "../api";
import { type PlainKey, t } from "../i18n";
import { Field, NumberInput } from "./ui";

export const CURVES: { value: FadeCurve; label: PlainKey }[] = [
  { value: "linear", label: "fade.linear" },
  { value: "s_curve", label: "fade.sCurve" },
  { value: "logarithmic", label: "fade.logarithmic" },
  { value: "exponential", label: "fade.exponential" },
];

/** One side of a crossfade: on or off, its curve, length and level. */
export function FadeEditor({
  value,
  onChange,
  kind,
  disabled,
}: Readonly<{
  value: Fade;
  onChange: (fade: Fade) => void;
  kind: "in" | "out";
  disabled?: boolean;
}>) {
  return (
    <div className="stack">
      <label className="check">
        <input
          type="checkbox"
          checked={value.enabled}
          disabled={disabled}
          onChange={(event) => onChange({ ...value, enabled: event.target.checked })}
        />
        {kind === "out" ? t("fade.outgoing") : t("fade.incoming")}
      </label>
      <div className="form-grid">
        <Field label={t("fade.curve")}>
          <select
            className="input"
            value={value.curve}
            disabled={disabled || !value.enabled}
            onChange={(event) => onChange({ ...value, curve: event.target.value as FadeCurve })}
          >
            {CURVES.map((curve) => (
              <option key={curve.value} value={curve.value}>
                {t(curve.label)}
              </option>
            ))}
          </select>
        </Field>
        <Field label={t("fade.length")} hint={kind === "out" ? t("fade.lengthOut") : t("fade.lengthIn")}>
          <NumberInput
            value={value.time_ms / 1000}
            min={0}
            max={30}
            step={0.1}
            suffix="s"
            disabled={disabled || !value.enabled}
            onChange={(seconds) => onChange({ ...value, time_ms: Math.round(seconds * 1000) })}
          />
        </Field>
        <Field label={kind === "out" ? t("fade.downTo") : t("fade.upFrom")} hint={t("fade.silence")}>
          <NumberInput
            value={value.level_pct}
            min={0}
            max={100}
            step={1}
            suffix="%"
            disabled={disabled || !value.enabled}
            onChange={(level) => onChange({ ...value, level_pct: Math.round(level) })}
          />
        </Field>
      </div>
    </div>
  );
}
