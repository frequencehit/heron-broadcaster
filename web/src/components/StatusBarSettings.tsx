// SPDX-License-Identifier: AGPL-3.0-or-later

// Account › Status bar (docs/web.md#status-bar): a live preview of the bar,
// at a computer's or a phone's width, and the user's settings, saved at
// each change.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { type ReactNode, useState } from "react";
import { api, unwrap } from "../api";
import { type Key, t } from "../i18n";
import { useModule } from "../lib/modules";
import { ME_KEY, useAction, useUser } from "../lib/session";
import { useStation } from "../lib/station";
import { DECK_TIMES, type StatusBarSettings, deckTime } from "../lib/statusbar";
import { StatusBar } from "./StatusBar";
import { Card, NumberInput, Segmented } from "./ui";

/** The parts that are a plain switch, in the bar's order (the decks and the history have their own choices). */
const SWITCHES = [
  { key: "listeners", label: "statusbarSettings.listeners", hint: "statusbarSettings.listenersHint" },
  { key: "next_fixed", label: "statusbarSettings.nextFixed", hint: "statusbarSettings.nextFixedHint" },
  { key: "queue", label: "statusbarSettings.queue", hint: "statusbarSettings.queueHint" },
  { key: "outputs", label: "statusbarSettings.outputs", hint: "statusbarSettings.outputsHint" },
  { key: "system", label: "statusbarSettings.system", hint: "statusbarSettings.systemHint" },
  { key: "station_time", label: "statusbarSettings.stationTime", hint: "statusbarSettings.stationTimeHint" },
] as const satisfies readonly { key: keyof StatusBarSettings; label: Key; hint: Key }[];

/** A colour of the theme, as `#rrggbb`, for a colour input showing the default. */
function themeColor(name: string, fallback: string): string {
  const value = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return /^#[0-9a-f]{6}$/i.test(value) ? value.toLowerCase() : fallback;
}

function Check({
  checked,
  onChange,
  disabled,
  children,
  hint,
}: Readonly<{ checked: boolean; onChange: (on: boolean) => void; disabled?: boolean; children: ReactNode; hint?: ReactNode }>) {
  return (
    <label className={`check statusbar-setting${disabled ? " disabled" : ""}`}>
      <input type="checkbox" checked={checked} disabled={disabled} onChange={(event) => onChange(event.target.checked)} />
      <span>
        {children}
        {hint && <span className="hint">{hint}</span>}
      </span>
    </label>
  );
}

function Countdown({
  label,
  hint,
  on,
  blink,
  seconds,
  onChange,
}: Readonly<{
  label: string;
  hint: string;
  on: boolean;
  blink: boolean;
  seconds: number;
  onChange: (change: { on?: boolean; blink?: boolean; seconds?: number }) => void;
}>) {
  return (
    <>
      <Check checked={on} onChange={(next) => onChange({ on: next })} hint={hint}>
        {label}
      </Check>
      <div className="statusbar-sub">
        <Check checked={blink} disabled={!on} onChange={(next) => onChange({ blink: next })}>
          {t("statusbarSettings.blink")}
        </Check>
        <div className={`row statusbar-seconds${on ? "" : " disabled"}`}>
          <NumberInput
            value={seconds}
            min={1}
            max={60}
            step={1}
            disabled={!on}
            label={t("statusbarSettings.secondsOf", { name: label })}
            onChange={(next) => onChange({ seconds: Math.round(next) })}
          />
          <span className="small">{t("statusbarSettings.lastSeconds")}</span>
        </div>
      </div>
    </>
  );
}

function ColorChoice({
  label,
  value,
  fallback,
  defaultLabel,
  onChange,
}: Readonly<{ label: string; value: string | null | undefined; fallback: string; defaultLabel: string; onChange: (color: string | null) => void }>) {
  return (
    <div className="row statusbar-color">
      <input
        type="color"
        aria-label={label}
        value={value ?? fallback}
        onChange={(event) => onChange(event.target.value.toLowerCase())}
      />
      <span>
        {label}
        <span className="hint">{value ? value : defaultLabel}</span>
      </span>
      {value && (
        <button type="button" className="btn small ghost" onClick={() => onChange(null)}>
          {t("statusbarSettings.defaultColor")}
        </button>
      )}
    </div>
  );
}

export function StatusBarSettingsCard() {
  const user = useUser();
  const client = useQueryClient();
  const module = useModule("status_bar");
  // The preview reads the station's live state: none before a station is chosen.
  const station = useStation();
  const [draft, setDraft] = useState<StatusBarSettings>(user.status_bar);
  const [phone, setPhone] = useState(false);
  const types = useQuery({ queryKey: ["category-types"], queryFn: () => unwrap(api.GET("/api/v1/category-types")) });
  const save = useAction(
    // A field left out takes its default: `{}` restores them all.
    (status_bar: Partial<StatusBarSettings>) =>
      unwrap(api.PUT("/api/v1/auth/preferences", { body: { status_bar: status_bar as StatusBarSettings } })),
    {
      onSuccess: (saved) => {
        client.setQueryData(ME_KEY, saved);
        setDraft(saved.status_bar);
      },
    },
  );
  const change = (patch: Partial<StatusBarSettings>) => {
    const next = { ...draft, ...patch };
    setDraft(next);
    save.mutate(next);
  };

  return (
    <Card
      title={t("statusbarSettings.title")}
      actions={
        <Check checked={draft.show} onChange={(show) => change({ show })}>
          {t("statusbarSettings.show")}
        </Check>
      }
    >
      <div className="stack statusbar-settings">
        {!module && <p className="muted small statusbar-module-off">{t("statusbarSettings.moduleOff")}</p>}
        <div className="stack" style={{ gap: "0.5rem" }}>
          <div className="row between">
            <h3 className="statusbar-group">{t("statusbarSettings.preview")}</h3>
            <Segmented
              label={t("statusbarSettings.previewWidth")}
              value={phone ? "phone" : "computer"}
              onChange={(next) => setPhone(next === "phone")}
              options={[
                { value: "computer", label: t("statusbarSettings.computer") },
                { value: "phone", label: t("statusbarSettings.phone") },
              ]}
            />
          </div>
          <div className="statusbar-preview-frame">
            <div style={{ width: phone ? "390px" : "100%", maxWidth: "100%", margin: "0 auto" }}>
              {draft.show ? (
                station !== null && <StatusBar settings={draft} preview />
              ) : (
                <p className="muted small statusbar-hidden">{t("statusbarSettings.hidden")}</p>
              )}
            </div>
          </div>
        </div>

        <div className="statusbar-settings-grid">
          <fieldset className="stack statusbar-fieldset">
            <legend className="statusbar-group">{t("statusbarSettings.parts")}</legend>
            <Check checked={draft.decks} onChange={(decks) => change({ decks })} hint={t("statusbarSettings.decksHint")}>
              {t("statusbarSettings.decks")}
            </Check>
            <div className="statusbar-sub" role="radiogroup" aria-label={t("statusbarSettings.decks")}>
              {[false, true].map((onAirOnly) => (
                <label key={String(onAirOnly)} className={`check${draft.decks ? "" : " disabled"}`}>
                  <input
                    type="radio"
                    name="statusbar-decks"
                    checked={draft.on_air_only === onAirOnly}
                    disabled={!draft.decks}
                    onChange={() => change({ on_air_only: onAirOnly })}
                  />
                  {t(onAirOnly ? "statusbarSettings.onAirOnly" : "statusbarSettings.bothDecks")}
                </label>
              ))}
            </div>
            <Check checked={draft.history} onChange={(history) => change({ history })} hint={t("statusbarSettings.historyHint")}>
              {t("statusbarSettings.history")}
            </Check>
            <div className="statusbar-sub">
              <label className={`row small${draft.history ? "" : " disabled"}`}>
                {t("statusbarSettings.historyType")}
                <select
                  className="input statusbar-select"
                  disabled={!draft.history}
                  value={draft.history_type_id ?? ""}
                  onChange={(event) =>
                    change({ history_type_id: event.target.value === "" ? null : Number(event.target.value) })
                  }
                >
                  {(types.data ?? []).map((type) => (
                    <option key={type.id} value={type.id}>
                      {type.name}
                    </option>
                  ))}
                  <option value="">{t("statusbarSettings.allTypes")}</option>
                </select>
              </label>
            </div>
            {SWITCHES.map((part) => (
              <Check
                key={part.key}
                checked={draft[part.key] as boolean}
                onChange={(on) => change({ [part.key]: on })}
                hint={t(part.hint)}
              >
                {t(part.label)}
              </Check>
            ))}
          </fieldset>

          <fieldset className="stack statusbar-fieldset">
            <legend className="statusbar-group">{t("statusbarSettings.time")}</legend>
            <div className="stack" role="radiogroup" aria-label={t("statusbarSettings.time")} style={{ gap: "0.35rem" }}>
              {DECK_TIMES.map((format) => (
                <label key={format} className={`statusbar-radio${draft.time === format ? " on" : ""}`}>
                  <input
                    type="radio"
                    name="statusbar-time"
                    checked={draft.time === format}
                    onChange={() => change({ time: format })}
                  />
                  {t(`statusbarSettings.time.${format}`)}
                  <code>{deckTime(format, 83_000, 312_000)}</code>
                </label>
              ))}
            </div>
            <p className="hint">{t("statusbarSettings.timeHint")}</p>
          </fieldset>

          <div className="stack">
            <fieldset className="stack statusbar-fieldset">
              <legend className="statusbar-group">{t("statusbarSettings.countdowns")}</legend>
              <p className="hint">{t("statusbarSettings.countdownsHint")}</p>
              <Countdown
                label={t("statusbarSettings.toVocals")}
                hint={t("statusbarSettings.toVocalsHint")}
                on={draft.intro_countdown}
                blink={draft.intro_blink}
                seconds={draft.intro_seconds}
                onChange={({ on, blink, seconds }) =>
                  change({
                    ...(on === undefined ? {} : { intro_countdown: on }),
                    ...(blink === undefined ? {} : { intro_blink: blink }),
                    ...(seconds === undefined ? {} : { intro_seconds: seconds }),
                  })
                }
              />
              <Countdown
                label={t("statusbarSettings.toNext")}
                hint={t("statusbarSettings.toNextHint")}
                on={draft.end_countdown}
                blink={draft.end_blink}
                seconds={draft.end_seconds}
                onChange={({ on, blink, seconds }) =>
                  change({
                    ...(on === undefined ? {} : { end_countdown: on }),
                    ...(blink === undefined ? {} : { end_blink: blink }),
                    ...(seconds === undefined ? {} : { end_seconds: seconds }),
                  })
                }
              />
              <p className="hint">{t("statusbarSettings.blinkHint")}</p>
            </fieldset>
            <fieldset className="stack statusbar-fieldset">
              <legend className="statusbar-group">{t("statusbarSettings.colors")}</legend>
              <ColorChoice
                label={t("statusbarSettings.introColor")}
                value={draft.intro_color}
                fallback={themeColor("--fx", "#b07a00")}
                defaultLabel={t("statusbarSettings.introColorDefault")}
                onChange={(intro_color) => change({ intro_color })}
              />
              <ColorChoice
                label={t("statusbarSettings.trackColor")}
                value={draft.track_color}
                fallback={themeColor("--deck-a", "#2f6fd6")}
                defaultLabel={t("statusbarSettings.trackColorDefault")}
                onChange={(track_color) => change({ track_color })}
              />
              <p className="hint">{t("statusbarSettings.colorsHint")}</p>
            </fieldset>
            <div>
              <button
                type="button"
                className="btn small"
                onClick={() => save.mutate({})}
                disabled={save.isPending}
              >
                {t("statusbarSettings.reset")}
              </button>
            </div>
          </div>
        </div>
        <p className="muted small" style={{ margin: 0 }}>
          {t("statusbarSettings.help")}
        </p>
      </div>
    </Card>
  );
}
