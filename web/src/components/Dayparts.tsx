// SPDX-License-Identifier: AGPL-3.0-or-later

// Daypart separation (docs/rotation.md#daypart-separation): the station's
// division and separation with an example over several days, a category's
// own value, and a pick's daypart in the rotation log.

import { useState } from "react";
import type { Explanation } from "../api";
import { t } from "../i18n";
import {
  DAYPART_COUNTS,
  type PartState,
  dayLabel,
  daypartHours,
  daypartLetter,
  daypartsList,
  describeSeparation,
  exampleDays,
  maxSeparation,
} from "../lib/dayparts";
import { dateTime } from "../lib/format";
import { Field, Segmented } from "./ui";

const STATES: Record<PartState, "dayparts.played" | "dayparts.kept" | "dayparts.free"> = {
  played: "dayparts.played",
  kept: "dayparts.kept",
  free: "dayparts.free",
};

/** The station's dayparts and their separation, with an example. */
export function DaypartSettings({
  count,
  separation,
  disabled,
  onChange,
}: Readonly<{
  count: number;
  separation: number;
  disabled: boolean;
  onChange: (count: number, separation: number) => void;
}>) {
  const [played, setPlayed] = useState(0);
  const max = maxSeparation(count);
  const from = Math.min(played, count - 1);
  const days = exampleDays(count, separation, from);
  return (
    <fieldset className="dayparts">
      <legend>{t("dayparts.title")}</legend>
      <p className="muted small" style={{ marginTop: 0 }}>
        {t("dayparts.help")}
      </p>
      <div className="form-grid">
        <div className="field">
          <span>{t("dayparts.division")}</span>
          <Segmented
            label={t("dayparts.division")}
            value={String(count)}
            disabled={disabled}
            options={DAYPART_COUNTS.map((n) => ({
              value: String(n),
              label: t("dayparts.divisionOption", { count: n, hours: 24 / n }),
            }))}
            onChange={(value) => {
              const next = Number(value);
              onChange(next, Math.min(separation, maxSeparation(next)));
            }}
          />
          <span className="hint">{daypartsList(count)}</span>
        </div>
        <div className="field">
          <span>{t("dayparts.separation")}</span>
          <Segmented
            label={t("dayparts.separation")}
            value={String(separation)}
            disabled={disabled}
            options={Array.from({ length: max + 1 }, (_, n) => ({ value: String(n), label: String(n) }))}
            onChange={(value) => onChange(count, Number(value))}
          />
          <span className="hint">{t("dayparts.separationHint", { max, last: count - 1 })}</span>
        </div>
      </div>
      <div className="stack daypart-example">
        <span className="small" style={{ fontWeight: 550 }}>{t("dayparts.example")}</span>
        <div className="daypart-days">
          {days.map((states, day) => (
            <DayRow key={day} day={day} count={count} states={states} onPick={day === 0 ? setPlayed : undefined} />
          ))}
          <span />
          <div className="daypart-hours" aria-hidden="true">
            {Array.from({ length: count + 1 }, (_, i) => (
              <span key={i}>{`${(i * 24) / count}h`}</span>
            ))}
          </div>
        </div>
        <div className="legend small muted">
          {(["played", "kept", "free"] as const).map((state) => (
            <span key={state}>
              <i className={`daypart-dot ${state}`} /> {t(STATES[state])}
            </span>
          ))}
        </div>
        <p className="small" style={{ margin: 0 }}>
          {describeSeparation(count, separation, from)}
        </p>
      </div>
    </fieldset>
  );
}

function DayRow({
  day,
  count,
  states,
  onPick,
}: Readonly<{
  day: number;
  count: number;
  states: PartState[];
  onPick: ((index: number) => void) | undefined;
}>) {
  const label = dayLabel(day);
  return (
    <>
      <span className="small muted">{label}</span>
      <div className="daypart-strip">
        {states.map((state, index) => {
          const letter = daypartLetter(index);
          const title = `${label} · ${letter}: ${t(STATES[state])}`;
          const content = (
            <>
              <b>{letter}</b>
              <small>{daypartHours(count, index)}</small>
            </>
          );
          return onPick ? (
            <button
              key={index}
              type="button"
              className={`daypart ${state}`}
              title={title}
              aria-pressed={state === "played"}
              onClick={() => onPick(index)}
            >
              {content}
            </button>
          ) : (
            <span key={index} className={`daypart ${state}`} title={title}>
              {content}
            </span>
          );
        })}
      </div>
    </>
  );
}

/** A category's daypart separation: the station's, or its own. */
export function CategoryDayparts({
  value,
  station,
  count,
  disabled,
  onChange,
}: Readonly<{
  /** Its own; `undefined` for the station's. */
  value: number | undefined;
  /** The station's, and its division of the day, when known. */
  station: number | undefined;
  count: number;
  disabled?: boolean;
  onChange: (value: number | undefined) => void;
}>) {
  const max = Math.max(maxSeparation(count), value ?? 0);
  const applied = value ?? station ?? 0;
  return (
    <Field label={t("dayparts.category")} hint={describeSeparation(count, applied)}>
      <select
        className="input"
        value={value === undefined ? "" : String(value)}
        disabled={disabled}
        onChange={(event) => onChange(event.target.value === "" ? undefined : Number(event.target.value))}
      >
        <option value="">{t("dayparts.stationValue", { value: station ?? "–" })}</option>
        {Array.from({ length: max + 1 }, (_, n) => (
          <option key={n} value={n}>
            {n === 0 ? t("dayparts.offOption") : n}
          </option>
        ))}
      </select>
    </Field>
  );
}

/** A pick's daypart: `C`, its hours in the title. */
export function DaypartBadge({ explanation }: Readonly<{ explanation: Explanation }>) {
  const daypart = explanation.daypart;
  if (!daypart) {
    return null;
  }
  const letter = daypartLetter(daypart.daypart);
  return (
    <span
      className="badge daypart-badge"
      title={`${t("dayparts.pickPart", { part: letter })} · ${daypartHours(daypart.count, daypart.daypart)}`}
    >
      {letter}
    </span>
  );
}

/**
 * What daypart separation did for a pick: where the track last played,
 * how far it was relaxed, how many tracks it held back. Nothing when off.
 */
export function daypartNote(explanation: Explanation): string | null {
  const daypart = explanation.daypart;
  if (!daypart || !explanation.enforce_rules || daypart.separation === 0) {
    return null;
  }
  const parts: string[] = [];
  const last = daypart.last_daypart;
  const step = explanation.step;
  if (step === 3 && last !== null && last !== undefined) {
    parts.push(
      t("dayparts.halved", {
        from: daypart.separation,
        to: explanation.windows.dayparts,
        played: daypartLetter(last),
        part: daypartLetter(daypart.daypart),
      }),
    );
  } else if (last !== null && last !== undefined && explanation.last_played_unix_ms) {
    parts.push(
      t("dayparts.lastIn", { part: daypartLetter(last), time: dateTime(explanation.last_played_unix_ms) }),
    );
  }
  const held = explanation.blocked.daypart ?? 0;
  if (held > 0 && step >= 3) {
    parts.push(t("rotationPage.blockedDaypart", { count: held }));
  }
  return parts.length > 0 ? parts.join("; ") : null;
}
