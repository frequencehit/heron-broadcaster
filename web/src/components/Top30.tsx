// SPDX-License-Identifier: AGPL-3.0-or-later

// The Top 30 (docs/listener-interaction.md#top-30): the chart of the week,
// live; the show, its programme and its new release; past weeks; its
// settings. And a track's rank in this week's chart, on its page.

import { useQuery } from "@tanstack/react-query";
import { type ReactNode, useState } from "react";
import { api, unwrap } from "../api";
import { locale, number, t } from "../i18n";
import { dateTime, duration, shortTime } from "../lib/format";
import { useModule } from "../lib/modules";
import { Link } from "../lib/router";
import { useAction, useCan } from "../lib/session";
import { here, useStation } from "../lib/station";
import {
  type Chart,
  type ChartEntry,
  type Top30Settings,
  countdown,
  leftOutLabel,
  movement,
  weekDays,
} from "../lib/top30";
import { DAY_NUMBERS, dayName } from "../lib/rotation";
import { ShowPage } from "../pages/Show";
import { LikedCell } from "./Likes";
import { Banner, Card, Empty, ErrorView, Field, Loading, NumberInput, Segmented } from "./ui";

type Tab = "chart" | "show" | "past" | "settings";

/** Shows › Top 30: its tabs, then the settings every show has. */
export function Top30Page() {
  const [tab, setTab] = useState<Tab>("chart");
  const tabs = (
    <>
      <Segmented
        value={tab}
        label={t("top30.tabs")}
        onChange={setTab}
        options={[
          { value: "chart", label: t("top30.tab.chart") },
          { value: "show", label: t("top30.tab.show") },
          { value: "past", label: t("top30.tab.past") },
          { value: "settings", label: t("top30.tab.settings") },
        ]}
      />
      {tab === "chart" && <ChartTab />}
      {tab === "show" && <ShowTab />}
      {tab === "past" && <PastWeeks />}
      {tab === "settings" && <SettingsCard />}
    </>
  );
  return <ShowPage module="top30" tabs={tabs} common={tab === "settings"} />;
}

function useChart(week: string | null) {
  const station = useStation();
  return useQuery({
    queryKey: ["top30", "chart", station, week],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/stations/{station_id}/top30/chart", {
          params: { path: here(), query: week ? { week } : {} },
        }),
      ),
    enabled: station !== null,
    refetchInterval: week ? false : 60_000,
  });
}

function useShowPlan() {
  const station = useStation();
  return useQuery({
    queryKey: ["top30", "show", station],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/top30/show", { params: { path: here() } })),
    enabled: station !== null,
    refetchInterval: 30_000,
  });
}

/** `2 – 8 November`. */
function weekLabel(week: string): string {
  const { monday, sunday } = weekDays(week);
  const format = new Intl.DateTimeFormat(locale(), { day: "numeric", month: "long", timeZone: "UTC" });
  return format.formatRange(new Date(monday), new Date(sunday));
}

/** This week's chart, after a closed chart whose show has not started yet. */
function ChartTab() {
  const plan = useShowPlan();
  const pending = plan.data && !plan.data.built && plan.data.week ? plan.data.week : null;
  const current = useChart(null);
  return (
    <>
      {pending && current.data && pending !== current.data.week && <ChartCard week={pending} />}
      <ChartCard week={null} />
    </>
  );
}

function Movement({ entry }: Readonly<{ entry: ChartEntry }>) {
  const { kind, places } = movement(entry);
  switch (kind) {
    case "up":
      return <span className="top30-move up">▲ {places}</span>;
    case "down":
      return <span className="top30-move down">▼ {places}</span>;
    case "same":
      return <span className="top30-move">=</span>;
    case "new":
      return <span className="top30-move word">{t("top30.new")}</span>;
    default:
      return <span className="top30-move word">{t("top30.reentry")}</span>;
  }
}

/** A week's chart: live until it closes, then frozen; tracks taken out and put back. */
export function ChartCard({ week }: Readonly<{ week: string | null }>) {
  const canEdit = useCan("operator");
  const chart = useChart(week);
  const remove = useAction(
    ({ chart, track, back }: { chart: Chart; track: number; back: boolean }) =>
      unwrap(
        back
          ? api.DELETE("/api/v1/stations/{station_id}/top30/charts/{week}/removals/{track_id}", {
              params: { path: here({ week: chart.week, track_id: track }) },
            })
          : api.POST("/api/v1/stations/{station_id}/top30/charts/{week}/removals/{track_id}", {
              params: { path: here({ week: chart.week, track_id: track }) },
            }),
      ),
    { invalidate: [["top30"]] },
  );
  if (chart.isPending) {
    return <Loading />;
  }
  if (chart.isError) {
    return <ErrorView error={chart.error} />;
  }
  const data = chart.data;
  const editable = canEdit && !data.locked;
  return (
    <Card
      title={
        <h2 className="row">
          {t("top30.weekOf", { range: weekLabel(data.week) })}
          {!data.frozen && <span className="badge good">{t("top30.live")}</span>}
        </h2>
      }
      actions={
        <span className="muted small">
          {t(data.frozen ? "top30.closed" : "top30.closes", { time: dateTime(data.closes_at_unix_ms) })} ·{" "}
          {t("likes.votes", { count: data.votes })}
        </span>
      }
      flush
    >
      {data.entries.length === 0 ? (
        <Empty>{t("top30.empty")}</Empty>
      ) : (
        <div className="table-wrap">
          <table className="table">
            <thead>
              <tr>
                <th className="num">{t("top30.rank")}</th>
                <th>{t("top30.movement")}</th>
                <th>{t("top30.track")}</th>
                <th>{t("likes.liked")}</th>
                <th className="num hide-narrow">{t("top30.weeks")}</th>
                {editable && <th />}
              </tr>
            </thead>
            <tbody>
              {data.entries.map((entry) => (
                <tr key={entry.rank}>
                  <td className="num">
                    <strong>{entry.rank}</strong>
                  </td>
                  <td>
                    <Movement entry={entry} />
                  </td>
                  <td>
                    {entry.track_id ? (
                      <Link to={`/library/${entry.track_id}`}>{label(entry)}</Link>
                    ) : (
                      label(entry)
                    )}
                  </td>
                  <td>
                    <LikedCell likes={entry.likes} dislikes={entry.dislikes} />
                  </td>
                  <td className="num hide-narrow">{entry.weeks}</td>
                  {editable && (
                    <td className="num">
                      {entry.track_id && (
                        <button
                          type="button"
                          className="btn small"
                          disabled={remove.isPending}
                          onClick={() => remove.mutate({ chart: data, track: entry.track_id ?? 0, back: false })}
                        >
                          {t("top30.takeOut")}
                        </button>
                      )}
                    </td>
                  )}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {data.removed.length > 0 && (
        <div className="card-body">
          <h3 className="small">{t("top30.removed")}</h3>
          <ul className="like-plays">
            {data.removed.map((removed) => (
              <li key={removed.track_id}>
                <span>
                  {[removed.artist, removed.title].filter(Boolean).join(" – ") || `#${removed.track_id}`}{" "}
                  <span className="muted small">
                    {t("top30.removedBy", {
                      user: removed.removed_by ?? "–",
                      time: dateTime(removed.removed_at_unix_ms),
                    })}
                  </span>
                </span>
                {editable && (
                  <button
                    type="button"
                    className="btn small"
                    disabled={remove.isPending}
                    onClick={() => remove.mutate({ chart: data, track: removed.track_id, back: true })}
                  >
                    {t("top30.putBack")}
                  </button>
                )}
              </li>
            ))}
          </ul>
        </div>
      )}
      {data.locked && <p className="muted small card-body">{t("top30.locked")}</p>}
    </Card>
  );
}

function label(entry: { artist?: string | null; title?: string | null; track_id?: number | null }): string {
  return [entry.artist, entry.title].filter(Boolean).join(" – ") || `#${entry.track_id ?? "?"}`;
}

/** The next show (or the one on air): its programme, its new release. */
function ShowTab() {
  const plan = useShowPlan();
  if (plan.isPending) {
    return <Loading />;
  }
  if (plan.isError) {
    return <ErrorView error={plan.error} />;
  }
  const show = plan.data;
  if (!show) {
    return (
      <Card>
        <Empty>{t("top30.noHours")}</Empty>
      </Card>
    );
  }
  const { tracks, from } = countdown(show);
  const slot = show.closes_at_unix_ms - show.opens_at_unix_ms;
  return (
    <Card
      title={t("top30.showOf", { time: dateTime(show.opens_at_unix_ms) })}
      actions={<span className="muted small">{show.built ? t("top30.built") : t("top30.preview")}</span>}
    >
      <div className="stats-tiles">
        <Tile value={number(tracks)} label={from ? t("top30.countdown", { from }) : t("top30.tracks")} />
        <Tile value={duration(show.music_ms)} label={t("top30.music")} />
        <Tile value={`+ ${duration(show.jingles_ms)}`} label={t("top30.jingles")} />
        <Tile value={`+ ${duration(show.new_release_ms)}`} label={t("top30.newRelease")} />
        <Tile value={duration(show.total_ms)} label={t("top30.ofSlot", { slot: duration(slot) })} />
      </div>
      {show.left_out.length > 0 && (
        <Banner kind="warn">{t("top30.leftOut", { ranks: leftOutLabel(show.left_out) })}</Banner>
      )}
      {show.week && <NewReleaseChoice week={show.week} locked={show.built} />}
      {show.items.length === 0 ? (
        <Empty>{t("top30.noProgramme")}</Empty>
      ) : (
        <div className="table-wrap">
          <table className="table top30-programme">
            <tbody>
              {show.items.map((item, index) => (
                <tr key={`${index}-${item.track_id}`} className={item.kind === "new_release" ? "top30-release" : ""}>
                  <td className="small">{item.skipped ? "" : shortTime(item.planned_at_unix_ms)}</td>
                  <td className="num">
                    {item.kind === "track" ? (
                      <strong>{item.rank}</strong>
                    ) : item.kind === "new_release" ? (
                      <span className="badge accent">{t("top30.new")}</span>
                    ) : null}
                  </td>
                  <td className={item.kind === "jingle" ? "muted small" : ""}>
                    {item.skipped ? <s>{label(item)}</s> : label(item)}
                    {item.skipped && <span className="badge warn">{t("top30.skipped", { why: item.skipped })}</span>}
                  </td>
                  <td className="num small">{duration(item.length_ms)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      <p className="muted small">{t("top30.rules")}</p>
    </Card>
  );
}

function Tile({ value, label }: Readonly<{ value: ReactNode; label: string }>) {
  return (
    <div className="stat">
      <span className="value">{value}</span>
      <span className="label">{label}</span>
    </div>
  );
}

/** The new release: proposed, or chosen among the tracks of its category. */
function NewReleaseChoice({ week, locked }: Readonly<{ week: string; locked: boolean }>) {
  const canEdit = useCan("operator");
  const chart = useChart(week);
  const settings = useSettings();
  const category = settings.data?.new_release_category_id ?? null;
  const tracks = useQuery({
    queryKey: ["tracks", "category", category],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/tracks", {
          params: { query: { category_id: category ?? 0, sort: "added", desc: true, limit: 200 } },
        }),
      ),
    enabled: category !== null,
  });
  const choose = useAction(
    (track: number | null) =>
      unwrap(
        api.PUT("/api/v1/stations/{station_id}/top30/charts/{week}/new-release", {
          params: { path: here({ week }) },
          body: { track_id: track },
        }),
      ),
    { invalidate: [["top30"]] },
  );
  if (category === null) {
    return <p className="muted small">{t("top30.noNewReleaseCategory")}</p>;
  }
  const release = chart.data?.new_release;
  return (
    <Field label={t("top30.newReleaseAfter")}>
      <span className="row">
        <select
          className="input"
          value={release?.track_id ?? ""}
          disabled={!canEdit || locked || choose.isPending}
          onChange={(e) => choose.mutate(e.target.value ? Number(e.target.value) : null)}
        >
          {release === null || release === undefined ? <option value="">–</option> : null}
          {release && !tracks.data?.items.some((track) => track.id === release.track_id) && (
            <option value={release.track_id}>{label(release)}</option>
          )}
          {tracks.data?.items.map((track) => (
            <option key={track.id} value={track.id}>
              {label({ artist: track.artist ?? null, title: track.title ?? null, track_id: track.id })}
            </option>
          ))}
        </select>
        {release?.proposed && <span className="badge">{t("top30.proposed")}</span>}
        {release && !release.proposed && canEdit && !locked && (
          <button type="button" className="btn small" onClick={() => choose.mutate(null)}>
            {t("top30.backToProposed")}
          </button>
        )}
      </span>
    </Field>
  );
}

/** The frozen charts, the latest first; one opens below. */
function PastWeeks() {
  const station = useStation();
  const [open, setOpen] = useState<string | null>(null);
  const charts = useQuery({
    queryKey: ["top30", "charts", station],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/stations/{station_id}/top30/charts", { params: { path: here(), query: { limit: 52 } } }),
      ),
    enabled: station !== null,
  });
  if (charts.isPending) {
    return <Loading />;
  }
  if (charts.isError) {
    return <ErrorView error={charts.error} />;
  }
  return (
    <>
      <Card title={t("top30.tab.past")}>
        {charts.data.length === 0 ? (
          <Empty>{t("top30.noPast")}</Empty>
        ) : (
          <ul className="like-plays">
            {charts.data.map((chart) => (
              <li key={chart.week}>
                <button type="button" className="link-button" onClick={() => setOpen(chart.week)}>
                  {t("top30.weekOf", { range: weekLabel(chart.week) })}
                </button>
                <span className="muted small">
                  {chart.number_one
                    ? t("top30.numberOne", { track: label(chart.number_one) })
                    : t("top30.empty")}
                </span>
              </li>
            ))}
          </ul>
        )}
      </Card>
      {open && <ChartCard week={open} />}
    </>
  );
}

function useSettings() {
  const station = useStation();
  return useQuery({
    queryKey: ["top30", "settings", station],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/top30/settings", { params: { path: here() } })),
    enabled: station !== null,
  });
}

/** The week's close, the votes and size, the categories of the chart and the show. */
function SettingsCard() {
  const settings = useSettings();
  if (settings.isPending) {
    return <Loading />;
  }
  if (settings.isError) {
    return <ErrorView error={settings.error} />;
  }
  return <SettingsForm initial={settings.data} />;
}

function SettingsForm({ initial }: Readonly<{ initial: Top30Settings }>) {
  const canEdit = useCan("operator");
  const station = useStation();
  const [draft, setDraft] = useState<Top30Settings>(initial);
  const categories = useQuery({
    queryKey: ["categories"],
    queryFn: () => unwrap(api.GET("/api/v1/categories")),
  });
  const save = useAction(
    (body: Top30Settings) =>
      unwrap(api.PUT("/api/v1/stations/{station_id}/top30/settings", { params: { path: here() }, body })),
    { success: t("categories.saved"), invalidate: [["top30"]] },
  );
  const ours = (categories.data ?? []).filter((category) => station !== null && category.station_ids.includes(station));
  const set = (change: Partial<Top30Settings>) => setDraft((current) => ({ ...current, ...change }));
  const dirty = JSON.stringify(draft) !== JSON.stringify(initial);
  const categorySelect = (value: number | null | undefined, onChange: (id: number | null) => void, name: string) => (
    <select
      className="input"
      aria-label={name}
      value={value ?? ""}
      disabled={!canEdit}
      onChange={(e) => onChange(e.target.value ? Number(e.target.value) : null)}
    >
      <option value="">{t("top30.none")}</option>
      {ours.map((category) => (
        <option key={category.id} value={category.id}>
          {category.name}
        </option>
      ))}
    </select>
  );
  return (
    <Card
      title={t("top30.settings")}
      actions={
        canEdit && (
          <button type="button" className="btn primary" disabled={!dirty || save.isPending} onClick={() => save.mutate(draft)}>
            {t("common.save")}
          </button>
        )
      }
    >
      <div className="grid-2">
        <Field label={t("top30.closes.label")} hint={t("top30.closes.hint")}>
          <span className="row">
            <select
              className="input"
              aria-label={t("top30.closeDay")}
              value={draft.close_day}
              disabled={!canEdit}
              onChange={(e) => set({ close_day: Number(e.target.value) })}
            >
              {DAY_NUMBERS.map((day) => (
                <option key={day} value={day}>
                  {dayName(day)}
                </option>
              ))}
            </select>
            <select
              className="input"
              aria-label={t("top30.closeHour")}
              value={draft.close_hour}
              disabled={!canEdit}
              onChange={(e) => set({ close_hour: Number(e.target.value) })}
            >
              {Array.from({ length: 25 }, (_, hour) => (
                <option key={hour} value={hour}>
                  {`${String(hour).padStart(2, "0")}:00`}
                </option>
              ))}
            </select>
          </span>
        </Field>
        <span className="row">
          <Field label={t("top30.minVotes")}>
            <NumberInput
              value={draft.min_votes}
              min={1}
              max={1000}
              step={1}
              disabled={!canEdit}
              label={t("top30.minVotes")}
              onChange={(min_votes) => set({ min_votes })}
            />
          </Field>
          <Field label={t("top30.size")}>
            <NumberInput
              value={draft.size}
              min={1}
              max={50}
              step={1}
              disabled={!canEdit}
              label={t("top30.size")}
              onChange={(size) => set({ size })}
            />
          </Field>
        </span>
      </div>
      <div className="field">
        <span>{t("top30.eligible")}</span>
        <span className="row" style={{ flexWrap: "wrap" }}>
          {ours.map((category) => (
            <label key={category.id} className="check">
              <input
                type="checkbox"
                disabled={!canEdit}
                checked={draft.category_ids.includes(category.id)}
                onChange={(e) =>
                  set({
                    category_ids: e.target.checked
                      ? [...draft.category_ids, category.id]
                      : draft.category_ids.filter((id) => id !== category.id),
                  })
                }
              />
              {category.name}
            </label>
          ))}
        </span>
        <span className="hint">{t("top30.eligibleHint")}</span>
      </div>
      <div className="grid-2">
        <Field label={t("top30.jingles")} hint={t("top30.jinglesHint")}>
          {categorySelect(draft.jingle_category_id, (id) => set({ jingle_category_id: id }), t("top30.jingles"))}
        </Field>
        <Field label={t("top30.newReleaseFrom")} hint={t("top30.newReleaseHint")}>
          {categorySelect(
            draft.new_release_category_id,
            (id) => set({ new_release_category_id: id }),
            t("top30.newReleaseFrom"),
          )}
        </Field>
      </div>
    </Card>
  );
}

/** "7th in this week's Top 30", on a track's page. */
export function Top30Rank({ trackId }: Readonly<{ trackId: number }>) {
  const on = useModule("top30");
  const station = useStation();
  const chart = useQuery({
    queryKey: ["top30", "chart", station, null],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/top30/chart", { params: { path: here() } })),
    enabled: on && station !== null,
  });
  const entry = chart.data?.entries.find((found) => found.track_id === trackId);
  if (!on || !entry) {
    return null;
  }
  return (
    <p className="small">
      <Link to="/shows/top30">{t("top30.rankOf", { rank: entry.rank })}</Link>
    </p>
  );
}
