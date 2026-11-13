// SPDX-License-Identifier: AGPL-3.0-or-later

// The Featured artist (docs/listener-interaction.md#featured-artist): its
// planning, an artist per week or per day; the day's times, with what
// plays at each; its own settings.

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { type Schemas, api, unwrap } from "../api";
import { t } from "../i18n";
import { fullDate, shortTime } from "../lib/format";
import { useAction, useCan } from "../lib/session";
import { here, useStation } from "../lib/station";
import { ShowPage } from "../pages/Show";
import { ArtistPicker } from "./ArtistPicker";
import { Card, ConfirmButton, Empty, ErrorView, Field, Loading, NumberInput, Segmented } from "./ui";

type Tab = "planning" | "today" | "settings";
type FeaturedSettings = Schemas["FeaturedSettings"];
type PlanningEntry = Schemas["PlanningEntry"];
type DayTime = Schemas["DayTime"];

/** The minutes of an hour it can play at. */
export const MINUTES = [0, 15, 30, 45] as const;

/** Shows › Featured artist: the planning, today, then the settings every show has. */
export function FeaturedArtistPage() {
  const [tab, setTab] = useState<Tab>("planning");
  const tabs = (
    <>
      <Segmented
        value={tab}
        label={t("featured.tabs")}
        onChange={setTab}
        options={[
          { value: "planning", label: t("featured.tab.planning") },
          { value: "today", label: t("featured.tab.today") },
          { value: "settings", label: t("featured.tab.settings") },
        ]}
      />
      {tab === "planning" && <PlanningCard />}
      {tab === "today" && <TodayCard />}
      {tab === "settings" && <SettingsCard />}
    </>
  );
  return <ShowPage module="featured_artist" tabs={tabs} common={tab === "settings"} />;
}

function useSettings() {
  const station = useStation();
  return useQuery({
    queryKey: ["featured", "settings", station],
    queryFn: () =>
      unwrap(api.GET("/api/v1/stations/{station_id}/featured-artist/settings", { params: { path: here() } })),
    enabled: station !== null,
  });
}

/** A planning date (`YYYY-MM-DD`), written in full. */
function dateLabel(date: string): string {
  return fullDate(Date.parse(`${date}T12:00:00Z`));
}

/** The planning: an artist per week, or per day, the current one first. */
export function PlanningCard() {
  const station = useStation();
  const canEdit = useCan("operator");
  const planning = useQuery({
    queryKey: ["featured", "planning", station],
    queryFn: () =>
      unwrap(api.GET("/api/v1/stations/{station_id}/featured-artist/planning", { params: { path: here() } })),
    enabled: station !== null,
  });
  const [choosing, setChoosing] = useState<string | null>(null);
  const plan = useAction(
    ({ date, artist_id }: { date: string; artist_id: number }) =>
      unwrap(
        api.PUT("/api/v1/stations/{station_id}/featured-artist/planning/{date}", {
          params: { path: here({ date }) },
          body: { artist_id },
        }),
      ),
    { invalidate: [["featured"]], onSuccess: () => setChoosing(null) },
  );
  const remove = useAction(
    (date: string) =>
      unwrap(
        api.DELETE("/api/v1/stations/{station_id}/featured-artist/planning/{date}", {
          params: { path: here({ date }) },
        }),
      ),
    { invalidate: [["featured"]] },
  );
  if (planning.isPending) {
    return <Loading />;
  }
  if (planning.isError) {
    return <ErrorView error={planning.error} />;
  }
  const byWeek = planning.data.by === "week";
  const missing = planning.data.entries.filter((entry) => entry.artist_id === null).length;
  return (
    <Card
      title={byWeek ? t("featured.planningWeeks") : t("featured.planningDays")}
      actions={missing > 0 && <span className="badge warn">{t("featured.missing", { count: missing })}</span>}
    >
      <ul className="list">
        {planning.data.entries.map((entry) => (
          <PlanningRow
            key={entry.date}
            entry={entry}
            byWeek={byWeek}
            canEdit={canEdit}
            choosing={choosing === entry.date}
            onChoose={() => setChoosing(choosing === entry.date ? null : entry.date)}
            onPick={(artist_id) => plan.mutate({ date: entry.date, artist_id })}
            onRemove={() => remove.mutate(entry.date)}
          />
        ))}
      </ul>
    </Card>
  );
}

function PlanningRow({
  entry,
  byWeek,
  canEdit,
  choosing,
  onChoose,
  onPick,
  onRemove,
}: Readonly<{
  entry: PlanningEntry;
  byWeek: boolean;
  canEdit: boolean;
  choosing: boolean;
  onChoose: () => void;
  onPick: (artist: number) => void;
  onRemove: () => void;
}>) {
  const when = byWeek ? t("featured.weekOf", { date: dateLabel(entry.date) }) : dateLabel(entry.date);
  return (
    <li className="stack" style={{ gap: 8 }}>
      <div className="queue-item">
        <div className="meta">
          <div className="ellipsis">
            {entry.artist ?? <span className="muted">{t("featured.noArtist")}</span>}
            {entry.current && (
              <span className="badge good" style={{ marginLeft: 8 }}>
                {byWeek ? t("featured.thisWeek") : t("featured.today")}
              </span>
            )}
          </div>
          <div className="muted small ellipsis">{when}</div>
        </div>
        {canEdit && (
          <div className="row" style={{ flexWrap: "nowrap" }}>
            <button type="button" className="btn small" aria-expanded={choosing} onClick={onChoose}>
              {entry.artist_id === null ? t("featured.choose") : t("featured.change")}
            </button>
            {entry.artist_id !== null && (
              <ConfirmButton
                className="btn small"
                question={t("featured.removeConfirm", { when })}
                confirm={t("featured.remove")}
                onConfirm={onRemove}
              >
                {t("featured.remove")}
              </ConfirmButton>
            )}
          </div>
        )}
      </div>
      {choosing && (
        <ArtistPicker
          exclude={entry.artist_id === null || entry.artist_id === undefined ? [] : [entry.artist_id]}
          actions={(artist) => (
            <button type="button" className="btn small primary" onClick={() => onPick(artist.id)}>
              {t("featured.pick")}
            </button>
          )}
        />
      )}
    </li>
  );
}

const STATUS_BADGE: Record<DayTime["status"], string> = {
  planned: "",
  queued: "good",
  played: "good",
  passed_over: "warn",
  none: "",
};

/** Today's times, with their jingle and track, or why nothing plays. */
export function TodayCard() {
  const station = useStation();
  const day = useQuery({
    queryKey: ["featured", "day", station],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/stations/{station_id}/featured-artist/day", {
          params: { path: here() },
        }),
      ),
    enabled: station !== null,
    refetchInterval: 15_000,
  });
  if (day.isPending) {
    return <Loading />;
  }
  if (day.isError) {
    return <ErrorView error={day.error} />;
  }
  const { artist, times } = day.data;
  return (
    <Card title={artist ? t("featured.todayOf", { artist }) : t("featured.todayNone")}>
      {times.length === 0 ? (
        <Empty>{t("featured.noTimes")}</Empty>
      ) : (
        <ul className="list">
          {times.map((time) => (
            <li key={time.at_unix_ms} className="queue-item">
              <span className="mono" style={{ minWidth: "4em" }}>
                {shortTime(time.at_unix_ms)}
              </span>
              <div className="meta">
                <div className="ellipsis">
                  {time.track ? (
                    [time.track.artist, time.track.title].filter(Boolean).join(" – ")
                  ) : (
                    <span className="muted">{t("featured.noTrack")}</span>
                  )}
                </div>
                <div className="muted small ellipsis">
                  {time.jingle && t("featured.after", { jingle: time.jingle.title ?? "" })}
                  {time.started_at_unix_ms !== null && time.started_at_unix_ms !== undefined &&
                    ` ${t("featured.startedAt", { time: shortTime(time.started_at_unix_ms) })}`}
                  {time.reason && ` ${reasonText(time.reason)}`}
                </div>
              </div>
              <span className={`badge ${STATUS_BADGE[time.status]}`}>{t(`featured.status.${time.status}`)}</span>
            </li>
          ))}
        </ul>
      )}
    </Card>
  );
}

/** The API's reason, in the reader's language when it is a known one. */
function reasonText(reason: string): string {
  if (reason.startsWith("no artist")) return t("featured.reason.noArtist");
  if (reason.startsWith("the artist has no")) return t("featured.reason.noMusic");
  if (reason.startsWith("taken out")) return t("featured.reason.takenOut");
  return t("featured.reason.nothing");
}

/** Its own settings (the settings every show has aside). */
export function SettingsCard() {
  const settings = useSettings();
  if (settings.isPending) {
    return <Loading />;
  }
  if (settings.isError) {
    return <ErrorView error={settings.error} />;
  }
  return <SettingsForm initial={settings.data} />;
}

function SettingsForm({ initial }: Readonly<{ initial: FeaturedSettings }>) {
  const canEdit = useCan("operator");
  const station = useStation();
  const [draft, setDraft] = useState<FeaturedSettings>(initial);
  const categories = useQuery({
    queryKey: ["categories"],
    queryFn: () => unwrap(api.GET("/api/v1/categories")),
  });
  const save = useAction(
    (body: FeaturedSettings) =>
      unwrap(api.PUT("/api/v1/stations/{station_id}/featured-artist/settings", { params: { path: here() }, body })),
    { success: t("categories.saved"), invalidate: [["featured"]] },
  );
  const ours = (categories.data ?? []).filter((category) => station !== null && category.station_ids.includes(station));
  const set = (change: Partial<FeaturedSettings>) => setDraft((current) => ({ ...current, ...change }));
  const minutes: number[] = draft.minutes ?? [0];
  const toggle = (minute: number) =>
    set({
      minutes: minutes.includes(minute)
        ? minutes.filter((other) => other !== minute)
        : [...minutes, minute].sort((a, b) => a - b),
    });
  const dirty = JSON.stringify(draft) !== JSON.stringify(initial);
  return (
    <Card
      title={t("featured.settings")}
      actions={
        canEdit && (
          <button
            type="button"
            className="btn primary"
            disabled={!dirty || minutes.length === 0 || save.isPending}
            onClick={() => save.mutate(draft)}
          >
            {t("common.save")}
          </button>
        )
      }
    >
      <div className="form-grid">
        {/* Not a <label>: its checkboxes keep their own names. */}
        <div className="field">
          <span>{t("featured.minutes")}</span>
          <div className="row" style={{ gap: "1.25rem" }}>
            {MINUTES.map((minute) => (
              <label key={minute} className="check">
                <input
                  type="checkbox"
                  checked={minutes.includes(minute)}
                  disabled={!canEdit}
                  onChange={() => toggle(minute)}
                />
                <span className="mono">:{String(minute).padStart(2, "0")}</span>
              </label>
            ))}
          </div>
          <span className="hint">{minutes.length === 0 ? t("featured.minutesNone") : t("featured.minutesHint")}</span>
        </div>
        <div className="field">
          <span>{t("featured.by")}</span>
          <Segmented
            label={t("featured.by")}
            value={draft.by ?? "week"}
            disabled={!canEdit}
            options={[
              { value: "week", label: t("featured.byWeek") },
              { value: "day", label: t("featured.byDay") },
            ]}
            onChange={(by) => set({ by })}
          />
          <span className="hint">{t("featured.byHint")}</span>
        </div>
        <Field label={t("featured.before")}>
          <NumberInput
            value={draft.window_before_s ?? 10}
            min={0}
            max={300}
            step={1}
            disabled={!canEdit}
            label={t("featured.before")}
            suffix={t("featured.seconds")}
            onChange={(window_before_s) => set({ window_before_s })}
          />
        </Field>
        <Field label={t("featured.after_")}>
          <NumberInput
            value={draft.window_after_s ?? 30}
            min={0}
            max={300}
            step={1}
            disabled={!canEdit}
            label={t("featured.after_")}
            suffix={t("featured.seconds")}
            onChange={(window_after_s) => set({ window_after_s })}
          />
        </Field>
        <Field label={t("featured.jingle")} hint={t("featured.jingleHint")}>
          <select
            className="input"
            aria-label={t("featured.jingle")}
            value={draft.jingle_category_id ?? ""}
            disabled={!canEdit}
            onChange={(e) => set({ jingle_category_id: e.target.value === "" ? null : Number(e.target.value) })}
          >
            <option value="">{t("featured.noJingle")}</option>
            {ours.map((c) => (
              <option key={c.id} value={c.id}>
                {c.name}
              </option>
            ))}
          </select>
        </Field>
      </div>
    </Card>
  );
}
