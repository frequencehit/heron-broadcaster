// SPDX-License-Identifier: AGPL-3.0-or-later

// Medleys: song hooks and liners picked from a recipe, checked and
// rearranged here, then mixed into a library file by heron-librarian.
// See docs/medleys.md.

import { StationBadges, StationChecks, useSeveralStations } from "../components/Stations";
import { here, useStation } from "../lib/station";
import { useQuery } from "@tanstack/react-query";
import { type FormEvent, useEffect, useState } from "react";
import { type Schemas, api, done, unwrap } from "../api";
import { MedleyTimeline, SHIFT_STEP_MS, junctions } from "../components/MedleyTimeline";
import { useLabels } from "../components/SelectionActions";
import { TaskState, useTask } from "../components/Task";
import { Card, ConfirmButton, Empty, ErrorView, Field, Icon, Loading, PageHead } from "../components/ui";
import { type PlainKey, number, rich, t } from "../i18n";
import { ago, duration, trackName } from "../lib/format";
import { displayName, layout, linerSlot, medleyName, renderDay } from "../lib/medley";
import { type PlayerSource, useMedleyPlayer } from "../lib/medleyPlayer";
import { Link, navigate } from "../lib/router";
import { useAction, useCan } from "../lib/session";

type Medley = Schemas["Medley"];
type Recipe = Schemas["Recipe"];
type PartView = Schemas["PartView"];

type MedleySettings = Schemas["MedleySettings"];

/** A new medley's recipe: the defaults of the settings, no songs yet. */
function newRecipe(settings: MedleySettings): Recipe {
  return { category_ids: [], label_id: null, order: "picked", liners: true, ...settings.defaults };
}

export function useMedleySettings() {
  return useQuery({
    queryKey: ["settings", "medleys"],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/settings/medleys", { params: { path: here() } })),
  });
}

function useMedleys() {
  return useQuery({
    queryKey: ["medleys"],
    queryFn: () => unwrap(api.GET("/api/v1/medleys")),
    // While a render runs, or its file waits for the library scan.
    refetchInterval: (query) =>
      query.state.data?.some(
        (medley) =>
          medley.task_status === "pending" ||
          medley.task_status === "running" ||
          (medley.path != null && medley.track_id == null),
      )
        ? 3_000
        : false,
  });
}

function useCategories() {
  return useQuery({ queryKey: ["categories"], queryFn: () => unwrap(api.GET("/api/v1/categories")) });
}

export function Medleys() {
  const canEdit = useCan("operator");
  const medleys = useMedleys();
  const station = useStation();
  const several = useSeveralStations();
  const [onlyHere, setOnlyHere] = useState(false);
  const shown = medleys.data?.filter(
    (medley) => !onlyHere || station === null || medley.station_ids.includes(station),
  );
  return (
    <>
      <PageHead
        title={t("nav.medleys")}
        description={t("medleys.description")}
        actions={
          canEdit && (
            <button type="button" className="btn primary" onClick={() => navigate("/medleys/new")}>
              <Icon name="plus" /> {t("medleys.new")}
            </button>
          )
        }
      />
      {several && (
        <label className="check">
          <input type="checkbox" checked={onlyHere} onChange={(e) => setOnlyHere(e.target.checked)} />{" "}
          {t("medleys.onlyStation")}
        </label>
      )}
      <Card flush>
        {medleys.isPending && <Loading />}
        {medleys.isError && <ErrorView error={medleys.error} />}
        {shown?.length === 0 && <Empty>{t("medleys.none")}</Empty>}
        {shown && shown.length > 0 && (
          <table className="table">
            <thead>
              <tr>
                <th>{t("medleys.name")}</th>
                <th>{t("medleys.hooks")}</th>
                <th>{t("medleys.rendered")}</th>
              </tr>
            </thead>
            <tbody>
              {shown.map((medley) => (
                <tr key={medley.id}>
                  <td>
                    <Link to={`/medleys/${medley.id}`}>{displayName(medley)}</Link>{" "}
                    <StationBadges ids={medley.station_ids} />
                  </td>
                  <td className="num">{number(medley.parts.filter((part) => part.kind === "hook").length)}</td>
                  <td className="small muted">
                    {medley.task_status === "failed"
                      ? t("medleys.renderFailed")
                      : medley.rendered_at_unix_ms
                        ? ago(medley.rendered_at_unix_ms)
                        : t("medleys.notRendered")}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </Card>
    </>
  );
}

/** One medley: its recipe, its current version, and building a new one. */
export function MedleyEditor({ id }: { id: number | null }) {
  const canEdit = useCan("operator");
  const medleys = useMedleys();
  const settings = useMedleySettings();
  const medley = id === null ? null : medleys.data?.find((m) => m.id === id);
  if ((id !== null && medleys.isPending) || settings.isPending) {
    return <Loading />;
  }
  if (settings.isError) {
    return <ErrorView error={settings.error} />;
  }
  if (id !== null && !medley) {
    return <ErrorView error={medleys.error ?? new Error(t("medleys.notFound"))} />;
  }
  return (
    <>
      <PageHead
        title={medley ? displayName(medley) : t("medleys.new")}
      />
      <RecipeCard key={medley?.id ?? "new"} medley={medley ?? null} settings={settings.data} canEdit={canEdit} />
      {medley && <BuildCard key={`build-${medley.id}`} medley={medley} settings={settings.data} canEdit={canEdit} />}
    </>
  );
}

function RecipeCard({
  medley,
  settings,
  canEdit,
}: {
  medley: Medley | null;
  settings: MedleySettings;
  canEdit: boolean;
}) {
  const categories = useCategories();
  const labels = useLabels();
  const station = useStation();
  const several = useSeveralStations();
  const [recipe, setRecipe] = useState<Recipe>(medley?.recipe ?? newRecipe(settings));
  const [stationIds, setStationIds] = useState<number[]>(
    medley?.station_ids ?? (station === null ? [] : [station]),
  );
  // The categories its stations can use.
  const offered = categories.data?.filter(
    (category) => !several || category.station_ids.some((id) => stationIds.includes(id)),
  );
  const set = (changes: Partial<Recipe>) => setRecipe((r) => ({ ...r, ...changes }));
  const save = useAction(
    () => {
      const body = { recipe, ...(several ? { station_ids: stationIds } : {}) };
      return medley
        ? unwrap(api.PUT("/api/v1/medleys/{id}", { params: { path: { id: medley.id } }, body }))
        : unwrap(api.POST("/api/v1/medleys", { body }));
    },
    {
      success: t("medleys.saved"),
      // The songs and liners to pick from follow the recipe.
      invalidate: [["medleys"], ["tracks"], ["track"], ["medley-candidates"]],
      onSuccess: (saved) => {
        if (!medley) {
          navigate(`/medleys/${saved.id}`, { replace: true });
        }
      },
    },
  );
  const remove = useAction(
    () => done(api.DELETE("/api/v1/medleys/{id}", { params: { path: { id: medley?.id ?? 0 } } })),
    { success: t("medleys.deleted"), invalidate: [["medleys"]], onSuccess: () => navigate("/medleys") },
  );
  const duplicate = useAction(
    () =>
      unwrap(
        api.POST("/api/v1/medleys/{id}/duplicate", {
          params: { path: { id: medley?.id ?? 0 } },
        }),
      ),
    {
      success: t("medleys.duplicated"),
      invalidate: [["medleys"]],
      onSuccess: (copy) => navigate(`/medleys/${copy.id}`),
    },
  );
  const disabled = !canEdit;
  const seconds = (ms: number) => String(ms / 1000);
  const fromSeconds = (text: string) => Math.round(Number(text.replace(",", ".")) * 1000);
  const submit = (event: FormEvent) => {
    event.preventDefault();
    save.mutate(undefined);
  };
  return (
    <Card
      title={t("medleys.recipe")}
      actions={
        canEdit &&
        medley && (
          <>
            <button
              type="button"
              className="btn small"
              title={t("medleys.duplicateHelp")}
              disabled={duplicate.isPending}
              onClick={() => duplicate.mutate(undefined)}
            >
              <Icon name="plus" size={14} /> {t("medleys.duplicate")}
            </button>
            <ConfirmButton
              className="btn small danger"
              question={t("medleys.deleteQuestion", { name: displayName(medley) })}
              confirm={t("medleys.delete")}
              onConfirm={() => remove.mutate(undefined)}
            >
              <Icon name="trash" size={14} /> {t("medleys.delete")}
            </ConfirmButton>
          </>
        )
      }
    >
      <form className="stack" onSubmit={submit}>
        <p className="small muted" style={{ margin: 0 }}>
          {rich("medleys.sharedHelp", {
            settings: <Link to="/settings">{t("medleys.settingsLink")}</Link>,
          })}
        </p>

        {several && (
          <Field label={t("medleys.stations")} hint={t("medleys.stationsHint")}>
            <StationChecks value={stationIds} onChange={setStationIds} disabled={disabled} />
          </Field>
        )}

        <fieldset>
          <legend>{t("medleys.songs")}</legend>
          <div className="stack">
            <div className="chips" role="group" aria-label={t("medleys.songCategories")}>
              {offered?.map((category) => (
                <label key={category.id} className="row small" style={{ gap: "0.3rem" }}>
                  <input
                    type="checkbox"
                    disabled={disabled}
                    checked={recipe.category_ids.includes(category.id)}
                    onChange={(event) =>
                      set({
                        category_ids: event.target.checked
                          ? [...recipe.category_ids, category.id]
                          : recipe.category_ids.filter((id) => id !== category.id),
                      })
                    }
                  />
                  {category.name}
                </label>
              ))}
            </div>
            <div className="form-grid">
              <Field label={t("medleys.label")}>
                <select
                  className="input"
                  value={recipe.label_id ?? ""}
                  disabled={disabled}
                  onChange={(event) => set({ label_id: event.target.value ? Number(event.target.value) : null })}
                >
                  <option value="">{t("medleys.anyLabel")}</option>
                  {labels.data?.map((label) => (
                    <option key={label.id} value={label.id}>
                      {label.name}
                    </option>
                  ))}
                </select>
              </Field>
              <Field label={t("medleys.order")}>
                <select
                  className="input"
                  value={recipe.order}
                  disabled={disabled}
                  onChange={(event) => set({ order: event.target.value as Recipe["order"] })}
                >
                  <option value="picked">{t("medleys.orderPicked")}</option>
                  <option value="year">{t("medleys.orderYear")}</option>
                  <option value="bpm">{t("medleys.orderBpm")}</option>
                </select>
              </Field>
            </div>
          </div>
        </fieldset>

        <fieldset>
          <legend>{t("medleys.hooks")}</legend>
          <div className="form-grid">
            <Field label={t("medleys.hookCount")}>
              <input
                className="input num"
                type="number"
                min={2}
                max={30}
                value={recipe.hooks}
                disabled={disabled}
                onChange={(event) => set({ hooks: Number(event.target.value) })}
              />
            </Field>
            <Field label={t("medleys.crossfade")}>
              <input
                className="input num"
                type="number"
                min={0}
                max={5}
                step={0.1}
                value={seconds(recipe.crossfade_ms)}
                disabled={disabled}
                onChange={(event) => set({ crossfade_ms: fromSeconds(event.target.value) })}
              />
            </Field>
            <Field label={t("medleys.hookMin")} hint={t("medleys.hookMinHelp")}>
              <input
                className="input num"
                type="number"
                min={1}
                max={60}
                step={0.5}
                value={seconds(recipe.hook_min_ms)}
                disabled={disabled}
                onChange={(event) => set({ hook_min_ms: fromSeconds(event.target.value) })}
              />
            </Field>
            <Field label={t("medleys.hookMax")} hint={t("medleys.hookMaxHelp")}>
              <input
                className="input num"
                type="number"
                min={1}
                max={60}
                step={0.5}
                value={seconds(recipe.hook_max_ms)}
                disabled={disabled}
                onChange={(event) => set({ hook_max_ms: fromSeconds(event.target.value) })}
              />
            </Field>
          </div>
        </fieldset>

        <fieldset>
          <legend>{t("medleys.liners")}</legend>
          <div className="stack">
            <label className="row small" style={{ gap: "0.3rem" }}>
              <input
                type="checkbox"
                checked={recipe.liners}
                disabled={disabled}
                onChange={(event) => set({ liners: event.target.checked })}
              />
              {t("medleys.withLiners")}
            </label>
            {recipe.liners && settings.liner_category_id == null && (
              <span className="small warn">{t("medleys.noLinerCategory")}</span>
            )}
            <div className="form-grid">
              <Field label={t("medleys.linerEvery")}>
                <input
                  className="input num"
                  type="number"
                  min={1}
                  max={30}
                  value={recipe.liner_every}
                  disabled={disabled || !recipe.liners}
                  onChange={(event) => set({ liner_every: Number(event.target.value) })}
                />
              </Field>
              <Field label={t("medleys.linerMode")}>
                <select
                  className="input"
                  value={recipe.liner_mode}
                  disabled={disabled || !recipe.liners}
                  onChange={(event) => set({ liner_mode: event.target.value as Recipe["liner_mode"] })}
                >
                  <option value="between">{t("medleys.linerBetween")}</option>
                  <option value="over">{t("medleys.linerOver")}</option>
                </select>
              </Field>
              {recipe.liner_mode === "over" && (
                <Field label={t("medleys.duck")}>
                  <input
                    className="input num"
                    type="number"
                    min={-40}
                    max={0}
                    step={1}
                    value={recipe.duck_db}
                    disabled={disabled || !recipe.liners}
                    onChange={(event) => set({ duck_db: Number(event.target.value) })}
                  />
                </Field>
              )}
              {recipe.liner_mode === "over" && (
                <>
                  <Field label={t("medleys.duckAttackAt")}>
                    <DuckPlaceSelect
                      ramp="attack"
                      value={recipe.duck_attack_at}
                      disabled={disabled || !recipe.liners}
                      onChange={(duck_attack_at) => set({ duck_attack_at })}
                    />
                  </Field>
                  <Field label={t("medleys.duckAttack")}>
                    <input
                      className="input num"
                      type="number"
                      min={0}
                      max={5}
                      step={0.1}
                      value={seconds(recipe.duck_attack_ms)}
                      disabled={disabled || !recipe.liners}
                      onChange={(event) => set({ duck_attack_ms: fromSeconds(event.target.value) })}
                    />
                  </Field>
                  <Field label={t("medleys.duckReleaseAt")}>
                    <DuckPlaceSelect
                      ramp="release"
                      value={recipe.duck_release_at}
                      disabled={disabled || !recipe.liners}
                      onChange={(duck_release_at) => set({ duck_release_at })}
                    />
                  </Field>
                  <Field label={t("medleys.duckRelease")}>
                    <input
                      className="input num"
                      type="number"
                      min={0}
                      max={5}
                      step={0.1}
                      value={seconds(recipe.duck_release_ms)}
                      disabled={disabled || !recipe.liners}
                      onChange={(event) => set({ duck_release_ms: fromSeconds(event.target.value) })}
                    />
                  </Field>
                </>
              )}
            </div>
            <div className="row">
              <label className="row small" style={{ gap: "0.3rem" }}>
                <input
                  type="checkbox"
                  checked={recipe.liner_at_start}
                  disabled={disabled || !recipe.liners}
                  onChange={(event) => set({ liner_at_start: event.target.checked })}
                />
                {t("medleys.linerAtStart")}
              </label>
              <label className="row small" style={{ gap: "0.3rem" }}>
                <input
                  type="checkbox"
                  checked={recipe.liner_at_end}
                  disabled={disabled || !recipe.liners}
                  onChange={(event) => set({ liner_at_end: event.target.checked })}
                />
                {t("medleys.linerAtEnd")}
              </label>
            </div>
          </div>
        </fieldset>

        <fieldset>
          <legend>{t("medleys.fadeIn")}</legend>
          <div className="form-grid">
            <Field label={t("medleys.fadeOutLength")} hint={t("medleys.fadeInHelp")}>
              <input
                className="input num"
                type="number"
                min={0}
                max={30}
                step={0.5}
                value={seconds(recipe.fade_in_ms)}
                disabled={disabled}
                onChange={(event) => set({ fade_in_ms: fromSeconds(event.target.value) })}
              />
            </Field>
            <Field label={t("medleys.fadeOutCurve")}>
              <FadeCurveSelect
                value={recipe.fade_in_curve}
                disabled={disabled || recipe.fade_in_ms === 0}
                onChange={(fade_in_curve) => set({ fade_in_curve })}
              />
            </Field>
          </div>
        </fieldset>

        <fieldset>
          <legend>{t("medleys.fadeOut")}</legend>
          <div className="form-grid">
            <Field label={t("medleys.fadeOutLength")} hint={t("medleys.fadeOutHelp")}>
              <input
                className="input num"
                type="number"
                min={0}
                max={30}
                step={0.5}
                value={seconds(recipe.fade_out_ms)}
                disabled={disabled}
                onChange={(event) => set({ fade_out_ms: fromSeconds(event.target.value) })}
              />
            </Field>
            <Field label={t("medleys.fadeOutCurve")}>
              <FadeCurveSelect
                value={recipe.fade_out_curve}
                disabled={disabled || recipe.fade_out_ms === 0}
                onChange={(fade_out_curve) => set({ fade_out_curve })}
              />
            </Field>
          </div>
        </fieldset>

        {canEdit && (
          <div className="row">
            <button
              type="submit"
              className="btn primary"
              disabled={save.isPending || (several && stationIds.length === 0)}
            >
              {t("common.save")}
            </button>
            {recipe.category_ids.length === 0 && (
              <span className="small muted">{t("medleys.allCategories")}</span>
            )}
          </div>
        )}
      </form>
    </Card>
  );
}

/** The curves of a fade-out. */
export function FadeCurveSelect({
  value,
  disabled,
  onChange,
}: {
  value: Recipe["fade_out_curve"];
  disabled: boolean;
  onChange: (curve: Recipe["fade_out_curve"]) => void;
}) {
  return (
    <select
      className="input"
      value={value}
      disabled={disabled}
      onChange={(event) => onChange(event.target.value as Recipe["fade_out_curve"])}
    >
      <option value="linear">{t("medleys.curveLinear")}</option>
      <option value="fast_then_slow">{t("medleys.curveFastThenSlow")}</option>
      <option value="slow_then_fast">{t("medleys.curveSlowThenFast")}</option>
      <option value="s_curve">{t("medleys.curveS")}</option>
    </select>
  );
}

/** Where the music goes down (`attack`), or comes back up (`release`), around a liner. */
export function DuckPlaceSelect({
  ramp,
  value,
  disabled,
  onChange,
}: {
  ramp: "attack" | "release";
  value: Recipe["duck_attack_at"];
  disabled: boolean;
  onChange: (place: Recipe["duck_attack_at"]) => void;
}) {
  const options: [Recipe["duck_attack_at"], PlainKey][] =
    ramp === "attack"
      ? [
          ["outside", "medleys.duckBefore"],
          ["across", "medleys.duckAcross"],
          ["inside", "medleys.duckDuring"],
        ]
      : [
          ["inside", "medleys.duckDuring"],
          ["across", "medleys.duckAcross"],
          ["outside", "medleys.duckAfter"],
        ];
  return (
    <select
      className="input"
      value={value}
      disabled={disabled}
      onChange={(event) => onChange(event.target.value as Recipe["duck_attack_at"])}
    >
      {options.map(([place, label]) => (
        <option key={place} value={place}>
          {t(label)}
        </option>
      ))}
    </select>
  );
}

function BuildCard({ medley, settings, canEdit }: { medley: Medley; settings: MedleySettings; canEdit: boolean }) {
  const player = useMedleyPlayer();
  // A new version being built; the current one otherwise.
  const [plan, setPlan] = useState<PartView[] | null>(null);
  const [qualified, setQualified] = useState<number | null>(null);
  const [taskId, setTaskId] = useState<number | null>(medley.task_id ?? null);
  const task = useTask(taskId);
  const build = useAction(
    () => unwrap(api.POST("/api/v1/medleys/{id}/plan", { params: { path: { id: medley.id } } })),
    {
      onSuccess: (result) => {
        setPlan(result.parts);
        setQualified(result.qualified);
      },
    },
  );
  const replace = useAction(
    async (index: number) => {
      const parts = plan ?? [];
      const part = parts[index];
      if (!part) {
        return null;
      }
      const other = await unwrap(
        api.POST("/api/v1/medleys/{id}/pick", {
          params: { path: { id: medley.id } },
          body: { kind: part.kind, exclude: parts.map((p) => p.track_id), slot: linerSlot(parts, index) },
        }),
      );
      return { index, other };
    },
    {
      onSuccess: (result) => {
        if (result) {
          setPlan((parts) => (parts ?? []).map((part, i) => (i === result.index ? result.other : part)));
        }
      },
    },
  );
  const render = useAction(
    () =>
      unwrap(
        api.POST("/api/v1/medleys/{id}/render", {
          params: { path: { id: medley.id } },
          body: {
            parts: (plan ?? []).map(({ kind, track_id, from_ms, to_ms, shift_ms, move_ms }) => ({
              kind,
              track_id,
              from_ms,
              to_ms,
              shift_ms,
              move_ms,
            })),
          },
        }),
      ),
    {
      success: t("medleys.renderAsked"),
      invalidate: [["medleys"]],
      onSuccess: (asked) => {
        // The file is about to change: nothing plays the old one meanwhile.
        player.stop();
        setTaskId(asked.id);
        setPlan(null);
      },
    },
  );
  // The songs and liners to choose from, while a new version is edited.
  const hookChoices = useCandidates(medley.id, "hook", "between", canEdit && plan !== null);
  const withLiners = canEdit && plan !== null && medley.recipe.liners;
  const linerChoices = {
    start: useCandidates(medley.id, "liner", "start", withLiners),
    between: useCandidates(medley.id, "liner", "between", withLiners),
    end: useCandidates(medley.id, "liner", "end", withLiners),
  };
  const choices = (kind: string, index: number) =>
    (kind === "hook" ? hookChoices.data : linerChoices[linerSlot(plan ?? [], index)].data) ?? [];
  const inPlan = new Set((plan ?? []).map((part) => part.track_id));
  const setPart = (index: number, chosen: PartView) =>
    setPlan((current) => (current ?? []).map((part, i) => (i === index ? chosen : part)));
  const addPart = (chosen: PartView) => setPlan((current) => [...(current ?? []), chosen]);

  // A mix of the version being edited, outside the library.
  const [previewTaskId, setPreviewTaskId] = useState<number | null>(null);
  const [previewed, setPreviewed] = useState<string | null>(null);
  const previewTask = useTask(previewTaskId);
  const asPartsBody = (parts: PartView[]) => ({
    parts: parts.map(({ kind, track_id, from_ms, to_ms, shift_ms, move_ms }) => ({
      kind,
      track_id,
      from_ms,
      to_ms,
      shift_ms,
      move_ms,
    })),
  });
  const mixPreview = useAction(
    () => {
      // What is sent now, whatever changes while it mixes; the last
      // preview is no longer playable meanwhile.
      setPreviewed(previewKey);
      setPreviewTaskId(null);
      return unwrap(
        api.POST("/api/v1/medleys/{id}/preview", {
          params: { path: { id: medley.id } },
          body: asPartsBody(plan ?? []),
        }),
      );
    },
    {
      onSuccess: (asked) => setPreviewTaskId(asked.id),
    },
  );
  const previewReady = previewTask.data?.status === "done";
  // What a preview was mixed from: the parts and the recipe saved (a recipe
  // saved since, a crossfade or the liners' placement, changes the mix too).
  const previewKey = JSON.stringify({ parts: asPartsBody(plan ?? []), recipe: medley.recipe });
  const previewStale = previewed !== null && previewed !== previewKey;
  const previewSource: PlayerSource | null =
    previewReady && previewTaskId !== null
      ? { kind: "medley", url: `/api/v1/medleys/${medley.id}/preview?task=${previewTaskId}` }
      : null;
  const renderedSource: PlayerSource | null =
    medley.track_id != null
      ? // Each render replaces the file at the same path: a new address for
        // each, or the browser plays the one it has kept.
        { kind: "medley", url: `/api/v1/tracks/${medley.track_id}/audio?render=${medley.rendered_at_unix_ms ?? 0}` }
      : null;
  // Where to play the preview from once it is mixed.
  const [pendingSeek, setPendingSeek] = useState<number | null>(null);
  const { play } = player;
  useEffect(() => {
    if (pendingSeek !== null && previewSource && !previewStale) {
      play(previewSource, pendingSeek);
      setPendingSeek(null);
    }
  }, [pendingSeek, previewSource?.url, previewStale, play]);
  /** Plays the medley from `at_ms`: the version being edited (mixed first when needed), or the rendered one. */
  const playFrom = (at_ms: number) => {
    if (plan) {
      if (previewSource && !previewStale) {
        player.play(previewSource, at_ms);
      } else if (canEdit && !mixPreview.isPending) {
        setPendingSeek(at_ms);
        mixPreview.mutate(undefined);
      }
    } else if (renderedSource) {
      player.play(renderedSource, at_ms);
    }
  };
  // Moves a part's start or end by `by` ms, within its file, keeping it at least 0.5 s long.
  const nudge = (index: number, end: "from_ms" | "to_ms", by: number) =>
    setPlan((current) =>
      (current ?? []).map((part, i) => {
        if (i !== index) {
          return part;
        }
        if (end === "from_ms") {
          return { ...part, from_ms: Math.max(0, Math.min(part.from_ms + by, part.to_ms - 500)) };
        }
        const limit = part.duration_ms ?? Number.POSITIVE_INFINITY;
        return { ...part, to_ms: Math.min(limit, part.from_ms + 120_000, Math.max(part.to_ms + by, part.from_ms + 500)) };
      }),
    );

  const status = task.data?.status;
  // While a render runs, the file and its layout are about to change: no
  // timeline and nothing to play until they have.
  const rendering = !plan && (status === "pending" || status === "running");
  // The version and its track change once rendered, and again once scanned.
  const { refetch } = useMedleys();
  useEffect(() => {
    if (status === "done" || status === "failed") {
      void refetch();
    }
  }, [status, refetch]);

  const parts = plan ?? medley.parts;
  // Shares a liner over the music between the hooks around it.
  const setShift = (index: number, shift_ms: number) =>
    setPlan((current) => (current ?? []).map((part, i) => (i === index ? { ...part, shift_ms } : part)));
  // Outside editing: the layout the file was rendered with, when saved for these parts.
  const saved = medley.rendered_layout;
  const rendered = !plan && saved && saved.parts.length === parts.length ? saved : null;
  const shown = rendered ?? layout(parts, medley.recipe);
  const placed = shown.parts;
  // The file playing does not last as long as the timeline says.
  const fileLength =
    !plan && player.source?.kind === "medley" && renderedSource && player.source.url === renderedSource.url
      ? player.length_ms
      : null;
  const outOfStep =
    !plan &&
    medley.track_id != null &&
    (rendered === null || (fileLength !== null && Math.abs(fileLength - shown.length_ms) > 250));
  const shares = new Map(
    junctions(parts, placed, medley.recipe.crossfade_ms).map((join) => [join.liner, join.share]),
  );
  // Moves a part in the medley.
  const setMove = (index: number, move_ms: number) =>
    setPlan((current) => (current ?? []).map((part, i) => (i === index ? { ...part, move_ms } : part)));
  const source = player.source;
  const playhead =
    source?.kind === "medley"
      ? player.position_ms
      : source?.kind === "part" && placed[source.index]
        ? (placed[source.index]?.at_ms ?? 0) + player.position_ms - (placed[source.index]?.from_ms ?? 0)
        : null;
  const medleySource = plan ? previewSource : renderedSource;
  const playingMedley = source?.kind === "medley" && medleySource !== null && source.url === medleySource.url;
  const move = (index: number, by: number) =>
    setPlan((current) => {
      const next = [...(current ?? [])];
      const [part] = next.splice(index, 1);
      if (part) {
        next.splice(index + by, 0, part);
      }
      return next;
    });
  const hooks = parts.filter((part) => part.kind === "hook").length;
  // The name the next render gives, while a version is edited or before the first.
  const nextName =
    plan || medley.name == null
      ? medleyName(
          renderDay(new Date()),
          parts
            .filter((part) => part.kind === "hook")
            .map((part) => (settings.name_from === "titles" ? part.title : part.artist)),
        )
      : null;

  return (
    <Card
      title={plan ? t("medleys.newVersion") : t("medleys.currentVersion")}
      actions={
        canEdit && (
          <>
            {plan && (
              <button type="button" className="btn small" onClick={() => setPlan(null)}>
                {t("track.revert")}
              </button>
            )}
            {!plan && medley.parts.length > 0 && (
              <button
                type="button"
                className="btn small"
                title={t("medleys.editHelp")}
                onClick={() => {
                  setPlan(medley.parts);
                  setQualified(null);
                }}
              >
                <Icon name="edit" size={14} /> {t("medleys.edit")}
              </button>
            )}
            {/* Rendered: edited, never picked anew (duplicate it for a new one). */}
            {medley.rendered_at_unix_ms == null && (
              <button type="button" className="btn small" disabled={build.isPending} onClick={() => build.mutate(undefined)}>
                {plan ? t("medleys.buildAgain") : t("medleys.build")}
              </button>
            )}
            {plan && (
              <button
                type="button"
                className="btn small"
                disabled={mixPreview.isPending || hooks === 0 || previewTask.data?.status === "pending" || previewTask.data?.status === "running"}
                onClick={() => {
                  setPendingSeek(0);
                  mixPreview.mutate(undefined);
                }}
              >
                <Icon name="play" size={14} /> {t("medleys.preview")}
              </button>
            )}
            {plan && (
              <button
                type="button"
                className="btn small primary"
                disabled={render.isPending || hooks === 0}
                onClick={() => render.mutate(undefined)}
              >
                {t("medleys.render")}
              </button>
            )}
          </>
        )
      }
      flush
    >
      <div className="card-head stack" style={{ gap: "0.3rem" }}>
        <span className="small muted">
          {t("medleys.summary", {
            hooks: number(hooks),
            liners: number(parts.length - hooks),
            length: duration(shown.length_ms),
          })}
          {plan && qualified !== null && qualified < medley.recipe.hooks && (
            <> · {t("medleys.fewSongs", { qualified: number(qualified), wanted: number(medley.recipe.hooks) })}</>
          )}
          {/* On the same line, added and removed: nothing below moves when it shows. */}
          {plan && previewStale && (
            <span className="warn">
              {" · "}
              {t("medleys.previewStale")}
            </span>
          )}
        </span>
        {nextName && hooks > 0 && (
          <span className="small muted">{t("medleys.nextName", { name: nextName })}</span>
        )}
        {!plan && <TaskState task={task.data} />}
        {/* While it mixes, the player says so on its own line; a failure shows here. */}
        {plan && previewTask.data?.status === "failed" && <TaskState task={previewTask.data} />}
        {parts.length > 0 && !rendering && (plan || medley.track_id != null) && (
          <div className="medley-player small" role="group" aria-label={t("medleys.player")}>
            <button
              type="button"
              className="btn small"
              disabled={plan ? !canEdit && !previewSource : false}
              onClick={() => {
                if (playingMedley && player.playing) {
                  player.pause();
                } else if (playingMedley) {
                  player.resume();
                } else {
                  playFrom(0);
                }
              }}
            >
              <Icon name={playingMedley && player.playing ? "pause" : "play"} size={14} />{" "}
              {playingMedley && player.playing
                ? t("medleys.pause")
                : plan
                  ? t("medleys.listenVersion")
                  : t("medleys.listenMedley")}
            </button>
            {source && (
              <button type="button" className="btn small" onClick={() => player.stop()}>
                {t("medleys.stop")}
              </button>
            )}
            {source && (
              <span className="num muted">
                {source.kind === "part"
                  ? trackName(parts[source.index] ?? { path: "" })
                  : `${duration(player.position_ms, true)} / ${duration(player.length_ms ?? shown.length_ms, true)}`}
              </span>
            )}
            {pendingSeek !== null && <span className="muted">{t("medleys.mixing")}</span>}
            {!plan && medley.track_id != null && <Link to={`/library/${medley.track_id}`}>{t("medleys.openTrack")}</Link>}
          </div>
        )}
        {outOfStep && <span className="small warn">{t("medleys.timelineOutOfStep")}</span>}
        {!plan && medley.path && medley.track_id == null && status === "done" && (
          <span className="small muted">{t("medleys.waitingScan")}</span>
        )}
      </div>
      {parts.length > 0 && !rendering && (
        <MedleyTimeline
          parts={parts}
          recipe={medley.recipe}
          rendered={rendered}
          playhead_ms={source ? playhead : null}
          onShift={canEdit && plan ? setShift : undefined}
          onMove={canEdit && plan ? setMove : undefined}
          onSeek={plan || renderedSource ? playFrom : undefined}
        />
      )}
      {parts.length === 0 ? (
        <Empty>{canEdit ? t("medleys.buildHelp") : t("medleys.notBuilt")}</Empty>
      ) : (
        <ol className="list">
          {parts.map((part, index) => {
            const playing = source?.kind === "part" && source.index === index && player.playing;
            return (
              <li key={`${index}-${part.track_id}`} className="queue-item">
                <span className="index">{index + 1}</span>
                <div className="meta">
                  <div className="ellipsis">
                    <Link to={`/library/${part.track_id}`}>{trackName(part)}</Link>
                  </div>
                  <div className="row small muted" style={{ gap: "0.4rem" }}>
                    <span className={part.kind === "hook" ? "badge accent" : "badge"}>
                      {part.kind === "hook" ? t("medleys.hook") : t("medleys.liner")}
                    </span>
                    {canEdit && plan ? (
                      <span className="num row" style={{ gap: "0.25rem", flexWrap: "nowrap" }}>
                        <Nudge label={t("medleys.startEarlier", { name: trackName(part) })} text="−" onClick={() => nudge(index, "from_ms", -100)} />
                        {duration(part.from_ms, true)}
                        <Nudge label={t("medleys.startLater", { name: trackName(part) })} text="+" onClick={() => nudge(index, "from_ms", 100)} />
                        <span aria-hidden="true">→</span>
                        <Nudge label={t("medleys.endEarlier", { name: trackName(part) })} text="−" onClick={() => nudge(index, "to_ms", -100)} />
                        {duration(part.to_ms, true)}
                        <Nudge label={t("medleys.endLater", { name: trackName(part) })} text="+" onClick={() => nudge(index, "to_ms", 100)} />
                        {" · "}
                        {duration(part.to_ms - part.from_ms, true)}
                      </span>
                    ) : (
                      <span className="num">
                        {duration(part.from_ms, true)} – {duration(part.to_ms, true)} ·{" "}
                        {duration(part.to_ms - part.from_ms, true)}
                      </span>
                    )}
                    {(part.move_ms ?? 0) !== 0 && (
                      <span className="num row" style={{ gap: "0.25rem", flexWrap: "nowrap" }}>
                        {canEdit && plan && (
                          <Nudge
                            label={t("medleys.moveEarlier", { name: trackName(part) })}
                            text="−"
                            onClick={() => setMove(index, (part.move_ms ?? 0) - SHIFT_STEP_MS)}
                          />
                        )}
                        {t("medleys.moved", { by: `${(part.move_ms ?? 0) > 0 ? "+" : "−"}${duration(Math.abs(part.move_ms ?? 0), true)}` })}
                        {canEdit && plan && (
                          <>
                            <Nudge
                              label={t("medleys.moveLater", { name: trackName(part) })}
                              text="+"
                              onClick={() => setMove(index, (part.move_ms ?? 0) + SHIFT_STEP_MS)}
                            />
                            <button
                              type="button"
                              className="btn small nudge"
                              aria-label={t("medleys.moveBack", { name: trackName(part) })}
                              title={t("medleys.moveBack", { name: trackName(part) })}
                              onClick={() => setMove(index, 0)}
                            >
                              ↺
                            </button>
                          </>
                        )}
                      </span>
                    )}
                    {shares.has(index) && (
                      <span className="num row" style={{ gap: "0.25rem", flexWrap: "nowrap" }}>
                        {canEdit && plan && (
                          <Nudge
                            label={t("medleys.moreAfter", { name: trackName(part) })}
                            text="−"
                            onClick={() => setShift(index, (part.shift_ms ?? 0) - SHIFT_STEP_MS)}
                          />
                        )}
                        {t("medleys.share", { before: String(Math.round((shares.get(index) ?? 0.5) * 100)) })}
                        {canEdit && plan && (
                          <Nudge
                            label={t("medleys.moreBefore", { name: trackName(part) })}
                            text="+"
                            onClick={() => setShift(index, (part.shift_ms ?? 0) + SHIFT_STEP_MS)}
                          />
                        )}
                      </span>
                    )}
                  </div>
                </div>
                <div className="row" style={{ flexWrap: "nowrap", gap: "0.25rem" }}>
                  {!rendering && (
                    <button
                      type="button"
                      className="btn ghost small icon"
                      aria-label={t(playing ? "library.stopPreviewNamed" : "library.previewNamed", { name: trackName(part) })}
                      aria-pressed={playing}
                      onClick={() =>
                        playing
                          ? player.stop()
                          : player.play({
                              kind: "part",
                              index,
                              track_id: part.track_id,
                              from_ms: part.from_ms,
                              to_ms: part.to_ms,
                            })
                      }
                    >
                      <Icon name={playing ? "stop" : "play"} />
                    </button>
                  )}
                  {canEdit && plan && (
                    <>
                      <ChoiceSelect
                        label={t("medleys.chooseFor", { name: trackName(part) })}
                        placeholder={part.kind === "hook" ? t("medleys.chooseSong") : t("medleys.chooseLiner")}
                        // A song once per medley; a liner anywhere but twice in a row.
                        choices={choices(part.kind, index).filter((choice) =>
                          part.kind === "hook" ? !inPlan.has(choice.track_id) : choice.track_id !== part.track_id,
                        )}
                        onChoose={(chosen) => setPart(index, chosen)}
                      />
                      <button
                        type="button"
                        className="btn ghost small icon"
                        title={t("medleys.replace")}
                        aria-label={t("medleys.replaceNamed", { name: trackName(part) })}
                        disabled={replace.isPending}
                        onClick={() => replace.mutate(index)}
                      >
                        <Icon name="undo" />
                      </button>
                      <button
                        type="button"
                        className="btn ghost small icon"
                        aria-label={t("medleys.moveUp", { name: trackName(part) })}
                        disabled={index === 0}
                        onClick={() => move(index, -1)}
                      >
                        ↑
                      </button>
                      <button
                        type="button"
                        className="btn ghost small icon"
                        aria-label={t("medleys.moveDown", { name: trackName(part) })}
                        disabled={index === parts.length - 1}
                        onClick={() => move(index, 1)}
                      >
                        ↓
                      </button>
                      <button
                        type="button"
                        className="btn ghost small icon danger"
                        aria-label={t("medleys.removeNamed", { name: trackName(part) })}
                        onClick={() => setPlan((current) => (current ?? []).filter((_, i) => i !== index))}
                      >
                        <Icon name="close" />
                      </button>
                    </>
                  )}
                </div>
              </li>
            );
          })}
        </ol>
      )}
      {canEdit && plan && (
        <div className="card-head row" style={{ justifyContent: "flex-start" }}>
          <ChoiceSelect
            label={t("medleys.addHook")}
            placeholder={t("medleys.addHook")}
            choices={hookChoices.data?.filter((choice) => !inPlan.has(choice.track_id)) ?? []}
            onChoose={addPart}
          />
          {medley.recipe.liners && (
            <ChoiceSelect
              label={t("medleys.addLiner")}
              placeholder={t("medleys.addLiner")}
              // Added last: after every hook.
              choices={linerChoices.end.data ?? []}
              onChoose={addPart}
            />
          )}
        </div>
      )}
    </Card>
  );
}

/** Moves a point by 0.1 s. */
function Nudge({ label, text, onClick }: { label: string; text: string; onClick: () => void }) {
  return (
    <button type="button" className="btn small nudge" aria-label={label} title={label} onClick={onClick}>
      {text}
    </button>
  );
}

function useCandidates(id: number, kind: "hook" | "liner", slot: "start" | "between" | "end", enabled: boolean) {
  return useQuery({
    queryKey: ["medley-candidates", id, kind, slot],
    enabled,
    queryFn: () =>
      unwrap(api.GET("/api/v1/medleys/{id}/candidates", { params: { path: { id }, query: { kind, slot } } })),
  });
}

/** A list of tracks to pick one from; it goes back to its placeholder once picked. */
function ChoiceSelect({
  label,
  placeholder,
  choices,
  onChoose,
}: {
  label: string;
  placeholder: string;
  choices: PartView[];
  onChoose: (chosen: PartView) => void;
}) {
  return (
    <select
      className="input"
      style={{ maxWidth: "16rem" }}
      aria-label={label}
      value=""
      disabled={choices.length === 0}
      onChange={(event) => {
        const chosen = choices.find((choice) => String(choice.track_id) === event.target.value);
        if (chosen) {
          onChoose(chosen);
        }
      }}
    >
      <option value="">{placeholder}</option>
      {choices.map((choice) => (
        <option key={choice.track_id} value={choice.track_id}>
          {trackName(choice)}
        </option>
      ))}
    </select>
  );
}
