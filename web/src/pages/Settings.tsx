// SPDX-License-Identifier: AGPL-3.0-or-later

// Station settings: autofill, rotation, time zone, programming details,
// medleys, listener statistics, analysis and library management.

import { useQuery } from "@tanstack/react-query";
import { type ReactNode, useEffect, useState } from "react";
import { type Schemas, api, unwrap } from "../api";
import { Card, ErrorView, Field, Icon, Loading, NumberInput, PageHead } from "../components/ui";
import { type PlainKey, number, rich, t } from "../i18n";
import { WINDOW_FIELDS } from "../lib/rotation";
import { Link } from "../lib/router";
import { useStationCategories } from "./Clocks";
import { useStations } from "../components/Stations";
import { here, useStation } from "../lib/station";
import { DuckPlaceSelect, FadeCurveSelect, useMedleySettings } from "./Medleys";
import { useAction, useCan } from "../lib/session";

export function Settings() {
  return (
    <>
      <PageHead title={t("nav.settings")} description={t("settings.description")} />
      <QueueCard />
      <RotationCard />
      <StationCard />
      <ProgrammingCard />
      <MedleysCard />
      <StatisticsCard />
      <LibraryCard />
      <AnalysisCard />
    </>
  );
}

/** A card editing one settings document, saved as a whole. */
function SettingsCard<T>({
  title,
  description,
  value,
  save,
  saving,
  canEdit,
  children,
}: {
  title: string;
  description?: ReactNode;
  value: T;
  save: (value: T) => void;
  saving: boolean;
  canEdit: boolean;
  children: (draft: T, set: (changes: Partial<T>) => void) => ReactNode;
}) {
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);
  const dirty = JSON.stringify(draft) !== JSON.stringify(value);
  return (
    <Card
      title={title}
      actions={
        canEdit && (
          <>
            {dirty && (
              <button type="button" className="btn small" onClick={() => setDraft(value)}>
                {t("track.revert")}
              </button>
            )}
            <button
              type="button"
              className="btn small primary"
              disabled={!dirty || saving}
              onClick={() => save(draft)}
            >
              {t("common.save")}
            </button>
          </>
        )
      }
    >
      <div className="stack">
        {description && (
          <p className="muted small" style={{ margin: 0 }}>
            {description}
          </p>
        )}
        {children(draft, (changes) => setDraft((d) => ({ ...d, ...changes })))}
      </div>
    </Card>
  );
}

function QueueCard() {
  const canEdit = useCan("operator");
  const settings = useQuery({
    queryKey: ["settings", "queue"],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/settings/queue", { params: { path: here() } })),
  });
  const save = useAction(
    (body: Schemas["QueueSettings"]) => unwrap(api.PUT("/api/v1/stations/{station_id}/settings/queue", { params: { path: here() }, body })),
    { success: t("categories.saved"), invalidate: [["settings", "queue"]] },
  );
  if (settings.isPending) {
    return <Loading />;
  }
  if (settings.isError) {
    return <ErrorView error={settings.error} />;
  }
  return (
    <SettingsCard
      title={t("nav.queue")}
      description={rich("settings.queueHelp", {
        schedule: <Link to="/schedule">{t("clocks.scheduleLink")}</Link>,
        clocks: <Link to="/clocks">{t("settings.clocksLink")}</Link>,
        log: <Link to="/rotation">{t("settings.logLink")}</Link>,
      })}
      value={settings.data}
      save={(value) => save.mutate(value)}
      saving={save.isPending}
      canEdit={canEdit}
    >
      {(draft, set) => (
        <div className="form-grid">
          <label className="check">
            <input
              type="checkbox"
              checked={draft.autofill}
              disabled={!canEdit}
              onChange={(e) => set({ autofill: e.target.checked })}
            />
            {t("source.rotation")}
          </label>
          <Field label={t("settings.minQueued")} hint={t("settings.minQueuedHint")}>
            <NumberInput
              value={draft.min_queued}
              min={1}
              max={50}
              step={1}
              disabled={!canEdit || !draft.autofill}
              onChange={(min_queued) => set({ min_queued: Math.round(min_queued) })}
            />
          </Field>
        </div>
      )}
    </SettingsCard>
  );
}

const WINDOW_LABELS: Record<string, PlainKey> = {
  album_min: "separation.album",
  artist_min: "separation.artist",
  title_min: "separation.title",
  track_min: "separation.track",
};

function RotationCard() {
  const canEdit = useCan("operator");
  const categories = useStationCategories();
  const stations = useStations();
  const station = useStation();
  const others = stations.filter((s) => s.id !== station);
  const settings = useQuery({
    queryKey: ["settings", "rotation"],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/settings/rotation", { params: { path: here() } })),
  });
  const save = useAction(
    (body: Schemas["RotationSettings"]) => unwrap(api.PUT("/api/v1/stations/{station_id}/settings/rotation", { params: { path: here() }, body })),
    { success: t("categories.saved"), invalidate: [["settings", "rotation"]] },
  );
  if (settings.isPending) {
    return <Loading />;
  }
  if (settings.isError) {
    return <ErrorView error={settings.error} />;
  }
  return (
    <SettingsCard
      title={t("settings.rotation")}
      description={t("settings.rotationHelp")}
      value={settings.data}
      save={(value) => save.mutate(value)}
      saving={save.isPending}
      canEdit={canEdit}
    >
      {(draft, set) => (
        <>
          <div className="form-grid">
            {WINDOW_FIELDS.map(([key]) => (
              <Field key={key} label={t(WINDOW_LABELS[key] ?? "separation.track")}>
                <NumberInput
                  value={draft.separation[key]}
                  min={0}
                  max={10_080}
                  step={1}
                  suffix={t("common.min")}
                  disabled={!canEdit}
                  onChange={(minutes) => set({ separation: { ...draft.separation, [key]: Math.round(minutes) } })}
                />
              </Field>
            ))}
          </div>
          <div className="form-grid">
            <Field label={t("settings.fallback")} hint={t("settings.fallbackHint")}>
              <select
                className="input"
                value={draft.fallback_category_id ?? ""}
                disabled={!canEdit}
                onChange={(e) => set({ fallback_category_id: e.target.value ? Number(e.target.value) : null })}
              >
                <option value="">{t("settings.wholeLibrary")}</option>
                {categories.data?.map((category) => (
                  <option key={category.id} value={category.id}>
                    {category.name}
                  </option>
                ))}
              </select>
            </Field>
            <Field label={t("settings.playFactor")} hint={t("settings.playFactorHint")}>
              <NumberInput
                value={draft.weights.play_factor}
                min={0.5}
                max={1}
                step={0.05}
                disabled={!canEdit}
                onChange={(play_factor) => set({ weights: { ...draft.weights, play_factor } })}
              />
            </Field>
            <Field label={t("settings.requestFactor")} hint={t("settings.requestFactorHint")}>
              <NumberInput
                value={draft.weights.request_factor}
                min={1}
                max={2}
                step={0.05}
                disabled={!canEdit}
                onChange={(request_factor) => set({ weights: { ...draft.weights, request_factor } })}
              />
            </Field>
          </div>
          <Field label={t("settings.keptOut")} hint={t("settings.keptOutHint")}>
            <span className="station-checks">
              {categories.data?.map((category) => (
                <label key={category.id} className="check">
                  <input
                    type="checkbox"
                    disabled={!canEdit}
                    checked={draft.kept_out_category_ids.includes(category.id)}
                    onChange={(event) =>
                      set({
                        kept_out_category_ids: event.target.checked
                          ? [...draft.kept_out_category_ids, category.id]
                          : draft.kept_out_category_ids.filter((id) => id !== category.id),
                      })
                    }
                  />{" "}
                  {category.name}
                </label>
              ))}
            </span>
          </Field>
          {others.length > 0 && (
            <fieldset>
              <legend>{t("settings.across")}</legend>
              <p className="muted small" style={{ marginTop: 0 }}>
                {t("settings.acrossHelp")}
              </p>
              <div className="form-grid">
                {(
                  [
                    ["artist_min", "separation.artist"],
                    ["title_min", "separation.title"],
                    ["track_min", "separation.track"],
                  ] as const
                ).map(([key, label]) => (
                  <Field key={key} label={t(label)} hint={t("settings.acrossOff")}>
                    <NumberInput
                      value={draft.across[key]}
                      min={0}
                      max={10_080}
                      step={1}
                      suffix={t("common.min")}
                      disabled={!canEdit}
                      onChange={(minutes) => set({ across: { ...draft.across, [key]: Math.round(minutes) } })}
                    />
                  </Field>
                ))}
              </div>
              <Field label={t("settings.acrossStations")} hint={t("settings.acrossStationsHint")}>
                <span className="station-checks">
                  {others.map((other) => (
                    <label key={other.id} className="check">
                      <input
                        type="checkbox"
                        disabled={!canEdit}
                        checked={draft.across.station_ids.includes(other.id)}
                        onChange={(event) =>
                          set({
                            across: {
                              ...draft.across,
                              station_ids: event.target.checked
                                ? [...draft.across.station_ids, other.id]
                                : draft.across.station_ids.filter((id) => id !== other.id),
                            },
                          })
                        }
                      />{" "}
                      {other.name} <span className="badge">{other.code}</span>
                    </label>
                  ))}
                </span>
              </Field>
            </fieldset>
          )}
        </>
      )}
    </SettingsCard>
  );
}

/** A category to pick, or none (`noneLabel`). */
function CategorySelect({
  value,
  noneLabel,
  disabled,
  onChange,
}: {
  value: number | null | undefined;
  noneLabel: string;
  disabled: boolean;
  onChange: (id: number | null) => void;
}) {
  const categories = useStationCategories();
  return (
    <select
      className="input"
      value={value ?? ""}
      disabled={disabled}
      onChange={(e) => onChange(e.target.value ? Number(e.target.value) : null)}
    >
      <option value="">{noneLabel}</option>
      {categories.data?.map((category) => (
        <option key={category.id} value={category.id}>
          {category.name}
        </option>
      ))}
    </select>
  );
}

function MedleysCard() {
  const canEdit = useCan("operator");
  const settings = useMedleySettings();
  const save = useAction(
    (body: Schemas["MedleySettings"]) => unwrap(api.PUT("/api/v1/stations/{station_id}/settings/medleys", { params: { path: here() }, body })),
    { success: t("categories.saved"), invalidate: [["settings", "medleys"], ["medley-candidates"]] },
  );
  if (settings.isPending) {
    return <Loading />;
  }
  if (settings.isError) {
    return <ErrorView error={settings.error} />;
  }
  const disabled = !canEdit;
  return (
    <SettingsCard
      title={t("nav.medleys")}
      description={rich("settings.medleysHelp", { medleys: <Link to="/medleys">{t("settings.medleysLink")}</Link> })}
      value={settings.data}
      save={(value) => save.mutate(value)}
      saving={save.isPending}
      canEdit={canEdit}
    >
      {(draft, set) => {
        const defaults = draft.defaults;
        const setDefaults = (changes: Partial<Schemas["MedleyDefaults"]>) =>
          set({ defaults: { ...defaults, ...changes } });
        return (
          <>
            <div className="form-grid">
              <Field label={t("settings.medleyCategory")} hint={t("settings.medleyCategoryHint")}>
                <CategorySelect
                  value={draft.category_id}
                  noneLabel={t("settings.noMedleyCategory")}
                  disabled={disabled}
                  onChange={(category_id) => set({ category_id })}
                />
              </Field>
              <Field label={t("settings.medleyNames")} hint={t("settings.medleyNamesHint")}>
                <select
                  className="input"
                  value={draft.name_from}
                  disabled={disabled}
                  onChange={(e) => set({ name_from: e.target.value as Schemas["NameFrom"] })}
                >
                  <option value="artists">{t("settings.namesFromArtists")}</option>
                  <option value="titles">{t("settings.namesFromTitles")}</option>
                </select>
              </Field>
            </div>
            <div className="form-grid">
              <Field label={t("settings.linerCategory")}>
                <CategorySelect
                  value={draft.liner_category_id}
                  noneLabel={t("settings.noLiners")}
                  disabled={disabled}
                  onChange={(liner_category_id) => set({ liner_category_id })}
                />
              </Field>
              <Field label={t("settings.openingLiners")} hint={t("settings.specialLinersHint")}>
                <CategorySelect
                  value={draft.opening_liner_category_id}
                  noneLabel={t("settings.sameLiners")}
                  disabled={disabled}
                  onChange={(opening_liner_category_id) => set({ opening_liner_category_id })}
                />
              </Field>
              <Field label={t("settings.closingLiners")} hint={t("settings.specialLinersHint")}>
                <CategorySelect
                  value={draft.closing_liner_category_id}
                  noneLabel={t("settings.sameLiners")}
                  disabled={disabled}
                  onChange={(closing_liner_category_id) => set({ closing_liner_category_id })}
                />
              </Field>
            </div>
            <fieldset>
              <legend>{t("settings.medleyDefaults")}</legend>
              <div className="stack">
                <div className="form-grid">
                  <Field label={t("medleys.hookCount")}>
                    <NumberInput
                      value={defaults.hooks}
                      min={2}
                      max={30}
                      step={1}
                      disabled={disabled}
                      onChange={(hooks) => setDefaults({ hooks: Math.round(hooks) })}
                    />
                  </Field>
                  <Field label={t("medleys.crossfade")}>
                    <NumberInput
                      value={defaults.crossfade_ms / 1000}
                      min={0}
                      max={5}
                      step={0.1}
                      disabled={disabled}
                      onChange={(s) => setDefaults({ crossfade_ms: Math.round(s * 1000) })}
                    />
                  </Field>
                  <Field label={t("medleys.hookMin")}>
                    <NumberInput
                      value={defaults.hook_min_ms / 1000}
                      min={1}
                      max={60}
                      step={0.5}
                      disabled={disabled}
                      onChange={(s) => setDefaults({ hook_min_ms: Math.round(s * 1000) })}
                    />
                  </Field>
                  <Field label={t("medleys.hookMax")}>
                    <NumberInput
                      value={defaults.hook_max_ms / 1000}
                      min={1}
                      max={60}
                      step={0.5}
                      disabled={disabled}
                      onChange={(s) => setDefaults({ hook_max_ms: Math.round(s * 1000) })}
                    />
                  </Field>
                  <Field label={t("medleys.linerEvery")}>
                    <NumberInput
                      value={defaults.liner_every}
                      min={1}
                      max={30}
                      step={1}
                      disabled={disabled}
                      onChange={(every) => setDefaults({ liner_every: Math.round(every) })}
                    />
                  </Field>
                  <Field label={t("medleys.linerMode")}>
                    <select
                      className="input"
                      value={defaults.liner_mode}
                      disabled={disabled}
                      onChange={(e) => setDefaults({ liner_mode: e.target.value as Schemas["LinerMode"] })}
                    >
                      <option value="between">{t("medleys.linerBetween")}</option>
                      <option value="over">{t("medleys.linerOver")}</option>
                    </select>
                  </Field>
                  <Field label={`${t("medleys.fadeIn")} (s)`}>
                    <NumberInput
                      value={defaults.fade_in_ms / 1000}
                      min={0}
                      max={30}
                      step={0.5}
                      disabled={disabled}
                      onChange={(s) => setDefaults({ fade_in_ms: Math.round(s * 1000) })}
                    />
                  </Field>
                  <Field label={t("medleys.fadeInCurve")}>
                    <FadeCurveSelect
                      value={defaults.fade_in_curve}
                      disabled={disabled || defaults.fade_in_ms === 0}
                      onChange={(fade_in_curve) => setDefaults({ fade_in_curve })}
                    />
                  </Field>
                  <Field label={`${t("medleys.fadeOut")} (s)`}>
                    <NumberInput
                      value={defaults.fade_out_ms / 1000}
                      min={0}
                      max={30}
                      step={0.5}
                      disabled={disabled}
                      onChange={(s) => setDefaults({ fade_out_ms: Math.round(s * 1000) })}
                    />
                  </Field>
                  <Field label={t("medleys.fadeOutCurveLong")}>
                    <FadeCurveSelect
                      value={defaults.fade_out_curve}
                      disabled={disabled || defaults.fade_out_ms === 0}
                      onChange={(fade_out_curve) => setDefaults({ fade_out_curve })}
                    />
                  </Field>
                  {defaults.liner_mode === "over" && (
                    <Field label={t("medleys.duck")}>
                      <NumberInput
                        value={defaults.duck_db}
                        min={-40}
                        max={0}
                        step={1}
                        disabled={disabled}
                        onChange={(duck_db) => setDefaults({ duck_db })}
                      />
                    </Field>
                  )}
                  {defaults.liner_mode === "over" && (
                    <>
                      <Field label={t("medleys.duckAttackAt")}>
                        <DuckPlaceSelect
                          ramp="attack"
                          value={defaults.duck_attack_at}
                          disabled={disabled}
                          onChange={(duck_attack_at) => setDefaults({ duck_attack_at })}
                        />
                      </Field>
                      <Field label={t("medleys.duckAttack")}>
                        <NumberInput
                          value={defaults.duck_attack_ms / 1000}
                          min={0}
                          max={5}
                          step={0.1}
                          disabled={disabled}
                          onChange={(s) => setDefaults({ duck_attack_ms: Math.round(s * 1000) })}
                        />
                      </Field>
                      <Field label={t("medleys.duckReleaseAt")}>
                        <DuckPlaceSelect
                          ramp="release"
                          value={defaults.duck_release_at}
                          disabled={disabled}
                          onChange={(duck_release_at) => setDefaults({ duck_release_at })}
                        />
                      </Field>
                      <Field label={t("medleys.duckRelease")}>
                        <NumberInput
                          value={defaults.duck_release_ms / 1000}
                          min={0}
                          max={5}
                          step={0.1}
                          disabled={disabled}
                          onChange={(s) => setDefaults({ duck_release_ms: Math.round(s * 1000) })}
                        />
                      </Field>
                    </>
                  )}
                </div>
                <div className="row">
                  <label className="check">
                    <input
                      type="checkbox"
                      checked={defaults.liner_at_start}
                      disabled={disabled}
                      onChange={(e) => setDefaults({ liner_at_start: e.target.checked })}
                    />
                    {t("medleys.linerAtStart")}
                  </label>
                  <label className="check">
                    <input
                      type="checkbox"
                      checked={defaults.liner_at_end}
                      disabled={disabled}
                      onChange={(e) => setDefaults({ liner_at_end: e.target.checked })}
                    />
                    {t("medleys.linerAtEnd")}
                  </label>
                </div>
              </div>
            </fieldset>
          </>
        );
      }}
    </SettingsCard>
  );
}

function timeZones(): string[] {
  try {
    return Intl.supportedValuesOf("timeZone");
  } catch {
    return [];
  }
}

function StationCard() {
  const isAdmin = useCan("admin");
  const settings = useQuery({
    queryKey: ["settings", "station"],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/settings/station", { params: { path: here() } })),
  });
  const save = useAction(
    (body: Schemas["StationSettings"]) => unwrap(api.PUT("/api/v1/stations/{station_id}/settings/station", { params: { path: here() }, body })),
    { success: t("categories.saved"), invalidate: [["settings", "station"]] },
  );
  if (settings.isPending) {
    return <Loading />;
  }
  if (settings.isError) {
    return <ErrorView error={settings.error} />;
  }
  return (
    <SettingsCard
      title={t("nav.station")}
      description={t("settings.stationHelp")}
      value={settings.data}
      save={(value) => save.mutate(value)}
      saving={save.isPending}
      canEdit={isAdmin}
    >
      {(draft, set) => (
        <Field label={t("settings.timeZone")} hint={t("settings.timeZoneHint")}>
          <input
            className="input"
            list="time-zones"
            value={draft.timezone}
            disabled={!isAdmin}
            onChange={(e) => set({ timezone: e.target.value })}
          />
          <datalist id="time-zones">
            {timeZones().map((zone) => (
              <option key={zone} value={zone} />
            ))}
          </datalist>
        </Field>
      )}
    </SettingsCard>
  );
}

function StatisticsCard() {
  const isAdmin = useCan("admin");
  const settings = useQuery({
    queryKey: ["settings", "statistics"],
    queryFn: () => unwrap(api.GET("/api/v1/settings/statistics")),
  });
  const save = useAction(
    (body: Schemas["StatisticsSettings"]) => unwrap(api.PUT("/api/v1/settings/statistics", { body })),
    { success: t("categories.saved"), invalidate: [["settings", "statistics"], ["listeners"]] },
  );
  if (settings.isPending) {
    return <Loading />;
  }
  if (settings.isError) {
    return <ErrorView error={settings.error} />;
  }
  return (
    <SettingsCard
      title={t("nav.statistics")}
      description={rich("settings.statisticsHelp", { page: <Link to="/statistics">{t("nav.statistics")}</Link> })}
      value={settings.data}
      save={(value) => save.mutate(value)}
      saving={save.isPending}
      canEdit={isAdmin}
    >
      {(draft, set) => (
        <div className="form-grid">
          <Field label={t("settings.minSession")} hint={t("settings.minSessionHint")}>
            <NumberInput
              value={draft.min_session_s}
              min={0}
              max={3600}
              step={1}
              suffix={t("settings.seconds")}
              disabled={!isAdmin}
              onChange={(min_session_s) => set({ min_session_s: Math.round(min_session_s) })}
            />
          </Field>
          <Field label={t("settings.retention")} hint={t("settings.retentionHint")}>
            <NumberInput
              value={draft.retention_days}
              min={7}
              max={3650}
              step={1}
              suffix={t("settings.days")}
              disabled={!isAdmin}
              onChange={(retention_days) => set({ retention_days: Math.round(retention_days) })}
            />
          </Field>
        </div>
      )}
    </SettingsCard>
  );
}

/** A scale typed as `soft, medium, hard`. */
function ScaleInput({
  value,
  onChange,
  disabled,
}: {
  value: string[];
  onChange: (value: string[]) => void;
  disabled: boolean;
}) {
  const [text, setText] = useState(value.join(", "));
  useEffect(() => {
    setText((current) => (split(current).join(",") === value.join(",") ? current : value.join(", ")));
  }, [value]);
  return (
    <input
      className="input"
      value={text}
      disabled={disabled}
      onChange={(event) => {
        setText(event.target.value);
        onChange(split(event.target.value));
      }}
    />
  );
}

function split(text: string): string[] {
  return text
    .split(",")
    .map((value) => value.trim())
    .filter(Boolean);
}

function ProgrammingCard() {
  const canEdit = useCan("operator");
  const settings = useQuery({
    queryKey: ["settings", "programming"],
    queryFn: () => unwrap(api.GET("/api/v1/settings/programming")),
  });
  const save = useAction(
    (body: Schemas["ProgrammingSettings"]) => unwrap(api.PUT("/api/v1/settings/programming", { body })),
    { success: t("categories.saved"), invalidate: [["settings", "programming"], ["tracks"], ["track"]] },
  );
  const details = useAction(
    (all: boolean) => unwrap(api.POST("/api/v1/library/musicbrainz-details", { body: { all } })),
    { invalidate: [["library-tasks"]] },
  );
  const tasks = useQuery({
    queryKey: ["library-tasks"],
    queryFn: () => unwrap(api.GET("/api/v1/library/tasks", { params: { query: { limit: 50 } } })),
    refetchInterval: (query) =>
      query.state.data?.some(
        (task) =>
          task.request.kind === "musicbrainz_details" && (task.status === "pending" || task.status === "running"),
      )
        ? 2000
        : false,
  });
  const lookup = tasks.data?.find((task) => task.request.kind === "musicbrainz_details");
  if (settings.isPending) {
    return <Loading />;
  }
  if (settings.isError) {
    return <ErrorView error={settings.error} />;
  }
  return (
    <>
      <SettingsCard
        title={t("settings.programming")}
        description={t("settings.programmingHelp")}
        value={settings.data}
        save={(value) => save.mutate(value)}
        saving={save.isPending}
        canEdit={canEdit}
      >
        {(draft, set) => (
          <>
            <div className="form-grid">
              <Field label={t("settings.minPlay")} hint={t("settings.minPlayHint")}>
                <NumberInput
                  value={draft.min_play_s}
                  min={0}
                  max={3600}
                  step={1}
                  suffix="s"
                  disabled={!canEdit}
                  onChange={(min_play_s) => set({ min_play_s })}
                />
              </Field>
              <Field label={t("settings.moods")} hint={t("settings.scaleHint")}>
                <ScaleInput value={draft.moods} disabled={!canEdit} onChange={(moods) => set({ moods })} />
              </Field>
              <Field label={t("settings.soundCodes")} hint={t("settings.scaleHint")}>
                <ScaleInput
                  value={draft.sound_codes}
                  disabled={!canEdit}
                  onChange={(sound_codes) => set({ sound_codes })}
                />
              </Field>
            </div>
            <fieldset className="stack">
              <legend>{t("settings.eras")}</legend>
              {draft.eras.length === 0 && (
                <p className="muted small" style={{ margin: 0 }}>
                  {t("settings.erasHelp")}
                </p>
              )}
              {draft.eras.map((era, index) => {
                const change = (changes: Partial<Schemas["Era"]>) =>
                  set({ eras: draft.eras.map((e, i) => (i === index ? { ...e, ...changes } : e)) });
                const year = (value: string) => (value.trim() === "" ? null : Number(value));
                return (
                  <div key={index} className="row" style={{ flexWrap: "nowrap" }}>
                    <input
                      className="input"
                      aria-label={t("settings.era")}
                      value={era.name}
                      disabled={!canEdit}
                      onChange={(event) => change({ name: event.target.value })}
                    />
                    <input
                      className="input num"
                      type="number"
                      aria-label={t("settings.eraFrom")}
                      placeholder={t("settings.eraFromPlaceholder")}
                      value={era.from_year ?? ""}
                      disabled={!canEdit}
                      onChange={(event) => change({ from_year: year(event.target.value) })}
                    />
                    <input
                      className="input num"
                      type="number"
                      aria-label={t("settings.eraTo")}
                      placeholder={t("settings.eraToPlaceholder")}
                      value={era.to_year ?? ""}
                      disabled={!canEdit}
                      onChange={(event) => change({ to_year: year(event.target.value) })}
                    />
                    {canEdit && (
                      <button
                        type="button"
                        className="btn ghost small icon danger"
                        aria-label={t("settings.removeEra", { name: era.name })}
                        onClick={() => set({ eras: draft.eras.filter((_, i) => i !== index) })}
                      >
                        <Icon name="close" />
                      </button>
                    )}
                  </div>
                );
              })}
              {canEdit && (
                <div>
                  <button
                    type="button"
                    className="btn small"
                    onClick={() => set({ eras: [...draft.eras, { name: "", from_year: null, to_year: null }] })}
                  >
                    <Icon name="plus" size={14} /> {t("settings.addEra")}
                  </button>
                </div>
              )}
            </fieldset>
          </>
        )}
      </SettingsCard>
      <Card title={t("settings.musicbrainz")}>
        <div className="stack">
          <p className="muted small" style={{ margin: 0 }}>
            {t("settings.musicbrainzHelp")}
          </p>
          {canEdit && (
            <div className="row">
              <button type="button" className="btn" disabled={details.isPending} onClick={() => details.mutate(false)}>
                {t("settings.lookUpNew")}
              </button>
              <button
                type="button"
                className="btn ghost"
                disabled={details.isPending}
                onClick={() => details.mutate(true)}
              >
                {t("settings.lookUpAll")}
              </button>
            </div>
          )}
          {lookup && (
            <p className="small" style={{ margin: 0 }}>
              {lookup.status === "pending" && t("settings.lookupPending")}
              {lookup.status === "running" && t("settings.lookupRunning")}
              {lookup.status === "failed" && (
                <span className="bad">{t("selection.failed", { error: lookup.error ?? "" })}</span>
              )}
              {lookup.status === "done" &&
                lookup.details &&
                t("settings.lookupDone", {
                  releases: number(lookup.details.releases),
                  tracks: number(lookup.details.tracks),
                })}
              {lookup.status === "done" &&
                (lookup.details?.failed ?? 0) > 0 &&
                t("settings.lookupFailed", { count: lookup.details?.failed ?? 0 })}
            </p>
          )}
        </div>
      </Card>
    </>
  );
}

function LibraryCard() {
  const isAdmin = useCan("admin");
  const settings = useQuery({
    queryKey: ["settings", "library"],
    queryFn: () => unwrap(api.GET("/api/v1/settings/library")),
  });
  const [key, setKey] = useState("");
  const save = useAction(
    (body: Schemas["LibrarySettingsPatch"]) => unwrap(api.PATCH("/api/v1/settings/library", { body })),
    { success: t("categories.saved"), invalidate: [["settings", "library"]], onSuccess: () => setKey("") },
  );
  if (settings.isPending) {
    return <Loading />;
  }
  if (settings.isError) {
    return <ErrorView error={settings.error} />;
  }
  const current = settings.data;
  return (
    <>
      <SettingsCard
        title={t("nav.library")}
        description={rich("settings.libraryHelp", {
          fields: <code>{"{album_artist} {artist} {album} {title} {year} {track:02} {disc} {disc-} {ext}"}</code>,
        })}
        value={{
          layout: current.layout,
          min_score: current.min_score,
          save_cover: current.save_cover,
          embed_cover: current.embed_cover,
          write_tags: current.write_tags,
          max_upload_mb: current.max_upload_mb,
          min_free_mb: current.min_free_mb,
          trash_days: current.trash_days,
        }}
        save={(value) => save.mutate(value)}
        saving={save.isPending}
        canEdit={isAdmin}
      >
        {(draft, set) => (
          <>
            <Field label={t("settings.layout")} hint={t("settings.layoutHint")}>
              <input
                className="input mono"
                value={draft.layout}
                disabled={!isAdmin}
                onChange={(e) => set({ layout: e.target.value })}
              />
            </Field>
            <div className="form-grid">
              <Field label={t("settings.minScore")} hint={t("settings.minScoreHint")}>
                <NumberInput
                  value={draft.min_score}
                  min={0}
                  max={1}
                  step={0.01}
                  disabled={!isAdmin}
                  onChange={(min_score) => set({ min_score })}
                />
              </Field>
              <label className="check">
                <input
                  type="checkbox"
                  checked={draft.save_cover}
                  disabled={!isAdmin}
                  onChange={(e) => set({ save_cover: e.target.checked })}
                />
                {t("settings.saveCover")}
              </label>
              <label className="check">
                <input
                  type="checkbox"
                  checked={draft.embed_cover}
                  disabled={!isAdmin}
                  onChange={(e) => set({ embed_cover: e.target.checked })}
                />
                {t("settings.embedCover")}
              </label>
            </div>
            <div className="stack" style={{ gap: "0.2rem" }}>
              <label className="check">
                <input
                  type="checkbox"
                  checked={draft.write_tags}
                  disabled={!isAdmin}
                  onChange={(e) => set({ write_tags: e.target.checked })}
                />
                {t("settings.writeTags")}
              </label>
              <span className="muted small">{t("settings.writeTagsHint")}</span>
            </div>
            <div className="form-grid">
              <Field label={t("settings.maxUpload")} hint={t("settings.maxUploadHint")}>
                <NumberInput
                  value={draft.max_upload_mb}
                  min={1}
                  step={1}
                  suffix={t("settings.mib")}
                  disabled={!isAdmin}
                  onChange={(max_upload_mb) => set({ max_upload_mb })}
                />
              </Field>
              <Field label={t("settings.minFree")} hint={t("settings.minFreeHint")}>
                <NumberInput
                  value={draft.min_free_mb}
                  min={0}
                  step={1}
                  suffix={t("settings.mib")}
                  disabled={!isAdmin}
                  onChange={(min_free_mb) => set({ min_free_mb })}
                />
              </Field>
              <Field label={t("settings.trashDays")} hint={t("settings.trashDaysHint")}>
                <NumberInput
                  value={draft.trash_days}
                  min={1}
                  max={3650}
                  step={1}
                  suffix={t("settings.days")}
                  disabled={!isAdmin}
                  onChange={(trash_days) => set({ trash_days })}
                />
              </Field>
            </div>
          </>
        )}
      </SettingsCard>
      <Card title={t("track.identification")}>
        <div className="stack">
          <p className="muted small" style={{ margin: 0 }}>
            {t("settings.acoustidHelp")}
          </p>
          <p style={{ margin: 0 }}>
            {current.acoustid_key_set ? (
              <span className="good">{t("settings.keySet")}</span>
            ) : (
              <span className="warn">{t("settings.noKey")}</span>
            )}
          </p>
          {isAdmin && (
            <div className="row">
              <label style={{ flex: "1 1 16rem" }}>
                <span className="visually-hidden">{t("settings.key")}</span>
                <input
                  className="input mono"
                  type="password"
                  autoComplete="off"
                  placeholder={current.acoustid_key_set ? t("settings.replaceKey") : t("settings.keyPlaceholder")}
                  value={key}
                  onChange={(e) => setKey(e.target.value)}
                />
              </label>
              <button
                type="button"
                className="btn primary"
                disabled={!key.trim() || save.isPending}
                onClick={() => save.mutate({ acoustid_key: key.trim() })}
              >
                {t("settings.saveKey")}
              </button>
              {current.acoustid_key_set && (
                <button type="button" className="btn danger" onClick={() => save.mutate({ acoustid_key: null })}>
                  {t("settings.removeKey")}
                </button>
              )}
            </div>
          )}
        </div>
      </Card>
    </>
  );
}

const ANALYSIS_FIELDS: {
  key: keyof Schemas["AnalysisSettings"];
  label: PlainKey;
  hint: PlainKey;
  unit: string;
  min: number;
  max: number;
  step: number;
  ms?: boolean;
}[] = [
  { key: "start_threshold_db", label: "settings.startThreshold", hint: "settings.startThresholdHint", unit: "dB", min: -80, max: 0, step: 1 },
  { key: "end_threshold_db", label: "settings.endThreshold", hint: "settings.endThresholdHint", unit: "dB", min: -80, max: 0, step: 1 },
  { key: "silence_ceiling_dbfs", label: "settings.silenceCeiling", hint: "settings.silenceCeilingHint", unit: "dBFS", min: -90, max: 0, step: 1 },
  { key: "blip_max_ms", label: "settings.blips", hint: "settings.blipsHint", unit: "ms", min: 0, max: 1000, step: 1 },
  { key: "max_internal_silence_ms", label: "settings.internalSilence", hint: "settings.internalSilenceHint", unit: "s", min: 1, max: 600, step: 1, ms: true },
  { key: "fade_trigger_db", label: "settings.fadeTrigger", hint: "settings.fadeTriggerHint", unit: "dB", min: -60, max: 0, step: 1 },
  { key: "fade_min_ms", label: "settings.fadeMin", hint: "settings.fadeMinHint", unit: "s", min: 0, max: 30, step: 0.1, ms: true },
  { key: "fade_max_ms", label: "settings.fadeMax", hint: "settings.fadeMaxHint", unit: "s", min: 0, max: 60, step: 0.1, ms: true },
];

function AnalysisCard() {
  const isAdmin = useCan("admin");
  const settings = useQuery({
    queryKey: ["settings", "analysis"],
    queryFn: () => unwrap(api.GET("/api/v1/settings/analysis")),
  });
  const save = useAction(
    (body: Schemas["AnalysisSettings"]) => unwrap(api.PUT("/api/v1/settings/analysis", { body })),
    { success: t("settings.analysisSaved"), invalidate: [["settings", "analysis"], ["library-status"]] },
  );
  if (settings.isPending) {
    return <Loading />;
  }
  if (settings.isError) {
    return <ErrorView error={settings.error} />;
  }
  return (
    <SettingsCard
      title={t("settings.analysis")}
      description={t("settings.analysisHelp")}
      value={settings.data}
      save={(value) => save.mutate(value)}
      saving={save.isPending}
      canEdit={isAdmin}
    >
      {(draft, set) => (
        <div className="form-grid">
          {ANALYSIS_FIELDS.map((field) => (
            <Field key={field.key} label={t(field.label)} hint={t(field.hint)}>
              <NumberInput
                value={field.ms ? draft[field.key] / 1000 : draft[field.key]}
                min={field.min}
                max={field.max}
                step={field.step}
                suffix={field.unit}
                disabled={!isAdmin}
                onChange={(value) => set({ [field.key]: field.ms ? Math.round(value * 1000) : value })}
              />
            </Field>
          ))}
        </div>
      )}
    </SettingsCard>
  );
}
