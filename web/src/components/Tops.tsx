// SPDX-License-Identifier: AGPL-3.0-or-later

// The tops (docs/listener-interaction.md#tops): the station's list of tops;
// per top its live chart, its show and its new release, its past charts
// and its settings. And a track's ranks in the live charts, on its page.

import { useQuery } from "@tanstack/react-query";
import { type ReactNode, useEffect, useState } from "react";
import { api, unwrap } from "../api";
import { number, t } from "../i18n";
import { dateTime, duration, fullDate, shortTime } from "../lib/format";
import { useModule } from "../lib/modules";
import { Link, navigate } from "../lib/router";
import { useAction, useCan } from "../lib/session";
import type { ShowSettings, ShowView } from "../lib/shows";
import { here, useStation } from "../lib/station";
import {
  type Chart,
  type ChartEntry,
  NTHS,
  type Nth,
  SIZES,
  type TopSettings,
  type TopSlot,
  type TopView,
  countdown,
  givesWayLabel,
  leftOutLabel,
  movement,
  newTopSettings,
  nextDates,
  nthOption,
  periodLabel,
  scoreLabel,
  scoreParts,
  slotDays,
  slotLabel,
  validSlot,
  validTime,
  weekdayName,
  weekdayShort,
} from "../lib/tops";
import { Notifications, type Update, Website } from "../pages/Show";
import { LikedCell } from "./Likes";
import { Banner, Card, ConfirmButton, Empty, ErrorView, Field, Loading, NumberInput, PageHead, Segmented } from "./ui";

type Tab = "chart" | "show" | "past" | "settings";

function useTops() {
  const station = useStation();
  return useQuery({
    queryKey: ["tops", station],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/tops", { params: { path: here() } })),
    enabled: station !== null,
    refetchInterval: 60_000,
  });
}

/** Shows › Tops: each top with its period, score and show. */
export function TopsPage() {
  const canEdit = useCan("operator");
  const tops = useTops();
  const create = useAction(
    () =>
      unwrap(
        api.POST("/api/v1/stations/{station_id}/tops", {
          params: { path: here() },
          body: newTopSettings(t("tops.newName")),
        }),
      ),
    { invalidate: [["tops"]], onSuccess: (top) => navigate(`/shows/top30/${top.id}?tab=settings`) },
  );
  return (
    <>
      <PageHead
        title={t("nav.top30")}
        description={t("tops.help")}
        actions={
          canEdit && (
            <button type="button" className="btn primary" disabled={create.isPending} onClick={() => create.mutate(undefined)}>
              {t("tops.new")}
            </button>
          )
        }
      />
      {tops.isPending ? (
        <Loading />
      ) : tops.isError ? (
        <ErrorView error={tops.error} />
      ) : tops.data.length === 0 ? (
        <Card>
          <Empty>{t("tops.empty")}</Empty>
        </Card>
      ) : (
        <div className="stack">
          {tops.data.map((top) => (
            <TopCard key={top.id} top={top} />
          ))}
        </div>
      )}
    </>
  );
}

function TopCard({ top }: Readonly<{ top: TopView }>) {
  const settings = top.settings;
  return (
    <article className="card top-card">
      <div className="top-size" aria-hidden="true">
        <span>TOP</span>
        <strong>{settings.size}</strong>
      </div>
      <div className="top-card-main">
        <div className="row" style={{ flexWrap: "wrap" }}>
          <h2>{settings.name}</h2>
          <span className={settings.enabled ? "badge good" : "badge"}>{settings.enabled ? t("tops.on") : t("tops.off")}</span>
        </div>
        {top.error ? (
          <Banner kind="bad">{t("tops.unreadable", { error: top.error })}</Banner>
        ) : (
          <dl className="top-card-facts">
            <div>
              <dt>{t("tops.period")}</dt>
              <dd>{periodLabel(settings)}</dd>
            </div>
            <div>
              <dt>{t("tops.score")}</dt>
              <dd>{scoreLabel(settings)}</dd>
            </div>
            <div>
              <dt>{t("tops.show")}</dt>
              <dd>{slotLabel(settings.slot)}</dd>
            </div>
          </dl>
        )}
        {top.gives_way.map((other) => (
          <p key={other.top_id} className="top-warning small">
            {givesWayLabel(other)}
          </p>
        ))}
      </div>
      <div className="top-card-next">
        <span className="muted small">{t("tops.nextShow")}</span>
        <strong>{top.next_show ? dateTime(top.next_show.opens_at_unix_ms) : t("tops.noNext")}</strong>
        <Link to={`/shows/top30/${top.id}`}>{t("tops.open")}</Link>
      </div>
    </article>
  );
}

function useTop(id: number) {
  const station = useStation();
  return useQuery({
    queryKey: ["tops", station, id],
    queryFn: () =>
      unwrap(api.GET("/api/v1/stations/{station_id}/tops/{top_id}", { params: { path: here({ top_id: id }) } })),
    enabled: station !== null,
  });
}

function firstTab(): Tab {
  const wanted = new URLSearchParams(window.location.search).get("tab");
  return wanted === "show" || wanted === "past" || wanted === "settings" ? wanted : "chart";
}

/** A top: its chart, its show, its past charts and its settings. */
export function TopPage({ id }: Readonly<{ id: number }>) {
  const [tab, setTab] = useState<Tab>(firstTab);
  const top = useTop(id);
  if (top.isPending) {
    return <Loading />;
  }
  if (top.isError) {
    return <ErrorView error={top.error} />;
  }
  const view = top.data;
  return (
    <>
      <p className="small">
        <Link to="/shows/top30">‹ {t("nav.top30")}</Link>
      </p>
      <PageHead title={view.settings.name} description={t("tops.zone", { zone: view.timezone })} />
      {view.error && <Banner kind="bad">{t("tops.unreadable", { error: view.error })}</Banner>}
      <Segmented
        value={tab}
        label={t("tops.tabs")}
        onChange={setTab}
        options={[
          { value: "chart", label: t("tops.tab.chart") },
          { value: "show", label: t("tops.tab.show") },
          { value: "past", label: t("tops.tab.past") },
          { value: "settings", label: t("tops.tab.settings") },
        ]}
      />
      {tab === "chart" && !view.error && <ChartTab top={view} />}
      {tab === "show" && !view.error && <ShowTab top={view} />}
      {tab === "past" && <PastCharts top={view} />}
      {tab === "settings" && <SettingsForm top={view} />}
    </>
  );
}

function useChart(top: number, key: string | null) {
  const station = useStation();
  return useQuery({
    queryKey: ["tops", station, top, "chart", key],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/stations/{station_id}/tops/{top_id}/chart", {
          params: { path: here({ top_id: top }), query: key ? { key } : {} },
        }),
      ),
    enabled: station !== null,
    refetchInterval: key ? false : 60_000,
  });
}

function useShowPlan(top: number) {
  const station = useStation();
  return useQuery({
    queryKey: ["tops", station, top, "show"],
    queryFn: () =>
      unwrap(api.GET("/api/v1/stations/{station_id}/tops/{top_id}/show", { params: { path: here({ top_id: top }) } })),
    enabled: station !== null,
    refetchInterval: 30_000,
  });
}

/** The live chart, after a frozen chart whose show has not started yet. */
function ChartTab({ top }: Readonly<{ top: TopView }>) {
  const plan = useShowPlan(top.id);
  const pending = plan.data && !plan.data.built && plan.data.key ? plan.data.key : null;
  const current = useChart(top.id, null);
  return (
    <>
      {pending && current.data && pending !== current.data.key && <ChartCard top={top} chartKey={pending} />}
      <ChartCard top={top} chartKey={null} />
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
      return <span className="top30-move word">{t("tops.isNew")}</span>;
    default:
      return <span className="top30-move word">{t("tops.reentry")}</span>;
  }
}

function label(entry: { artist?: string | null; title?: string | null; track_id?: number | null }): string {
  return [entry.artist, entry.title].filter(Boolean).join(" – ") || `#${entry.track_id ?? "?"}`;
}

const percent = (value: number) => number(value, { style: "percent", maximumFractionDigits: 0 });

/** The score as a bar of its two parts, the plays' and the likes', and its points. */
function ScoreBar({ entry, playsPercent }: Readonly<{ entry: ChartEntry; playsPercent: number }>) {
  const parts = scoreParts(entry, playsPercent);
  return (
    <span className="top-score" title={t("tops.scoreTitle", { plays: Math.round(parts.plays), likes: Math.round(parts.likes) })}>
      <span className="top-score-bar" aria-hidden="true">
        <span className="plays" style={{ width: `${parts.plays}%` }} />
        <span className="likes" style={{ width: `${parts.likes}%` }} />
      </span>
      <strong>{number(Math.round(parts.score))}</strong>
    </span>
  );
}

/** A chart: live until it is frozen; tracks taken out and put back. */
export function ChartCard({ top, chartKey }: Readonly<{ top: TopView; chartKey: string | null }>) {
  const canEdit = useCan("operator");
  const chart = useChart(top.id, chartKey);
  const remove = useAction(
    ({ chart, track, back }: { chart: Chart; track: number; back: boolean }) => {
      const params = { params: { path: here({ top_id: top.id, key: chart.key, track_id: track }) } };
      return unwrap(
        back
          ? api.DELETE("/api/v1/stations/{station_id}/tops/{top_id}/charts/{key}/removals/{track_id}", params)
          : api.POST("/api/v1/stations/{station_id}/tops/{top_id}/charts/{key}/removals/{track_id}", params),
      );
    },
    { invalidate: [["tops"]] },
  );
  if (chart.isPending) {
    return <Loading />;
  }
  if (chart.isError) {
    return <ErrorView error={chart.error} />;
  }
  const data = chart.data;
  const editable = canEdit && !data.locked;
  const freezes = data.frozen
    ? t("tops.frozen", { time: dateTime(data.to_unix_ms) })
    : top.settings.period === "rolling_7_days"
      ? t("tops.freezesShow")
      : t("tops.freezes", { time: dateTime(data.to_unix_ms) });
  const row = (entry: ChartEntry, outside: boolean) => (
    <tr key={entry.rank} className={outside ? "top-outside" : ""}>
      <td className="num">
        <strong>{entry.rank}</strong>
      </td>
      <td>
        <Movement entry={entry} />
      </td>
      <td>{entry.track_id ? <Link to={`/library/${entry.track_id}`}>{label(entry)}</Link> : label(entry)}</td>
      <td className="num">
        {number(entry.plays)}
        {entry.plays_share !== null && entry.plays_share !== undefined && (
          <div className="muted small">{percent(entry.plays_share)}</div>
        )}
      </td>
      <td>
        <LikedCell likes={entry.likes} dislikes={entry.dislikes} />
      </td>
      <td>
        <ScoreBar entry={entry} playsPercent={data.plays_percent} />
      </td>
      {editable && (
        <td className="num">
          {entry.track_id && !outside && (
            <button
              type="button"
              className="btn small"
              disabled={remove.isPending}
              onClick={() => remove.mutate({ chart: data, track: entry.track_id ?? 0, back: false })}
            >
              {t("tops.takeOut")}
            </button>
          )}
        </td>
      )}
    </tr>
  );
  return (
    <Card
      title={
        <h2 className="row">
          {t("tops.range", { from: fullDate(data.from_unix_ms), to: dateTime(data.to_unix_ms) })}
          {!data.frozen && <span className="badge good">{t("tops.live")}</span>}
        </h2>
      }
      actions={
        <span className="muted small">
          {freezes} · {scoreLabel({ plays_percent: data.plays_percent, min_votes: data.min_votes })} ·{" "}
          {t("likes.votes", { count: data.votes })}
        </span>
      }
      flush
    >
      {data.entries.length === 0 ? (
        <Empty>{t("tops.noRanks")}</Empty>
      ) : (
        <div className="table-wrap">
          <table className="table top-chart">
            <thead>
              <tr>
                <th className="num">{t("tops.rank")}</th>
                <th>{t("tops.movement")}</th>
                <th>{t("tops.track")}</th>
                <th className="num">{t("tops.plays")}</th>
                <th>{t("tops.likes")}</th>
                <th>{t("tops.scoreColumn")}</th>
                {editable && <th aria-label={t("tops.action")} />}
              </tr>
            </thead>
            <tbody>
              {data.entries.map((entry) => row(entry, false))}
              {data.runners_up.map((entry) => row(entry, true))}
            </tbody>
          </table>
        </div>
      )}
      <div className="card-body">
        <p className="muted small row" style={{ gap: 16, flexWrap: "wrap" }}>
          <span className="row">
            <span className="top-key plays" /> {t("tops.legendPlays")}
          </span>
          <span className="row">
            <span className="top-key likes" /> {t("tops.legendLikes")}
          </span>
          {data.runners_up.length > 0 && <span>{t("tops.runnersUp", { size: data.size })}</span>}
        </p>
        {data.not_enough_votes.length > 0 && (
          <section className="top-not-enough">
            <h3 className="small">{t("tops.notEnough", { count: data.min_votes })}</h3>
            <ul className="like-plays">
              {data.not_enough_votes.map((track) => (
                <li key={track.track_id}>
                  <span>
                    {label(track)}{" "}
                    <span className="muted small">
                      {t("tops.notEnoughLine", {
                        plays: track.plays,
                        votes: track.likes + track.dislikes,
                        share: percent(track.share),
                        rank: track.would_rank,
                      })}
                    </span>
                  </span>
                </li>
              ))}
            </ul>
          </section>
        )}
        {data.removed.length > 0 && (
          <>
            <h3 className="small">{t("tops.removed")}</h3>
            <ul className="like-plays">
              {data.removed.map((removed) => (
                <li key={removed.track_id}>
                  <span>
                    {[removed.artist, removed.title].filter(Boolean).join(" – ") || `#${removed.track_id}`}{" "}
                    <span className="muted small">
                      {t("tops.removedBy", {
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
                      {t("tops.putBack")}
                    </button>
                  )}
                </li>
              ))}
            </ul>
          </>
        )}
        {data.locked && <p className="muted small">{t("tops.locked")}</p>}
      </div>
    </Card>
  );
}

/** The next show (or the one on air): its programme, its new release. */
function ShowTab({ top }: Readonly<{ top: TopView }>) {
  const plan = useShowPlan(top.id);
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
        <Empty>{t("tops.noSlot")}</Empty>
      </Card>
    );
  }
  const { tracks, from } = countdown(show);
  const slot = show.closes_at_unix_ms - show.opens_at_unix_ms;
  return (
    <Card
      title={t("tops.showOf", { time: dateTime(show.opens_at_unix_ms) })}
      actions={<span className="muted small">{show.built ? t("tops.built") : t("tops.preview")}</span>}
    >
      <div className="stats-tiles">
        <Tile value={number(tracks)} label={from ? t("tops.countdown", { from }) : t("tops.tracks")} />
        <Tile value={duration(show.music_ms)} label={t("tops.music")} />
        <Tile value={`+ ${duration(show.jingles_ms)}`} label={t("tops.jingles")} />
        <Tile value={`+ ${duration(show.new_release_ms)}`} label={t("tops.newRelease")} />
        <Tile value={duration(show.total_ms)} label={t("tops.ofSlot", { slot: duration(slot) })} />
      </div>
      {show.left_out.length > 0 && <Banner kind="warn">{t("tops.leftOut", { ranks: leftOutLabel(show.left_out) })}</Banner>}
      {show.key && <NewReleaseChoice top={top} chartKey={show.key} locked={show.built} />}
      {show.items.length === 0 ? (
        <Empty>{t("tops.noProgramme")}</Empty>
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
                      <span className="badge accent">{t("tops.isNew")}</span>
                    ) : null}
                  </td>
                  <td className={item.kind === "jingle" ? "muted small" : ""}>
                    {item.skipped ? <s>{label(item)}</s> : label(item)}
                    {item.skipped && <span className="badge warn">{t("tops.skipped", { why: item.skipped })}</span>}
                  </td>
                  <td className="num small">{duration(item.length_ms)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      <p className="muted small">{t("tops.rules")}</p>
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
function NewReleaseChoice({ top, chartKey, locked }: Readonly<{ top: TopView; chartKey: string; locked: boolean }>) {
  const canEdit = useCan("operator");
  const chart = useChart(top.id, chartKey);
  const category = top.settings.new_release_category_id ?? null;
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
        api.PUT("/api/v1/stations/{station_id}/tops/{top_id}/charts/{key}/new-release", {
          params: { path: here({ top_id: top.id, key: chartKey }) },
          body: { track_id: track },
        }),
      ),
    { invalidate: [["tops"]] },
  );
  if (category === null) {
    return <p className="muted small">{t("tops.noNewReleaseCategory")}</p>;
  }
  const release = chart.data?.new_release;
  return (
    <Field label={t("tops.newReleaseAfter")}>
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
        {release?.proposed && <span className="badge">{t("tops.proposed")}</span>}
        {release && !release.proposed && canEdit && !locked && (
          <button type="button" className="btn small" onClick={() => choose.mutate(null)}>
            {t("tops.backToProposed")}
          </button>
        )}
      </span>
    </Field>
  );
}

/** The frozen charts, the latest first; one opens below. */
function PastCharts({ top }: Readonly<{ top: TopView }>) {
  const station = useStation();
  const [open, setOpen] = useState<string | null>(null);
  const charts = useQuery({
    queryKey: ["tops", station, top.id, "charts"],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/stations/{station_id}/tops/{top_id}/charts", {
          params: { path: here({ top_id: top.id }), query: { limit: 52 } },
        }),
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
      <Card title={t("tops.tab.past")}>
        {charts.data.length === 0 ? (
          <Empty>{t("tops.noPast")}</Empty>
        ) : (
          <ul className="like-plays">
            {charts.data.map((chart) => (
              <li key={chart.key}>
                <button type="button" className="link-button" onClick={() => setOpen(chart.key)}>
                  {t("tops.range", { from: fullDate(chart.from_unix_ms), to: dateTime(chart.to_unix_ms) })}
                </button>
                <span className="muted small">
                  {chart.number_one ? t("tops.numberOne", { track: label(chart.number_one) }) : t("tops.noRanks")}
                </span>
              </li>
            ))}
          </ul>
        )}
      </Card>
      {open && <ChartCard top={top} chartKey={open} />}
    </>
  );
}

const DEFAULT_SLOT: TopSlot = { every: "week", weekdays: [7], nth: null, start: "18:00", end: "20:00" };
const DEFAULT_WEBSITE: ShowSettings["website"] = {
  title: "",
  color: "#c62828",
  small_widget: true,
  closed: "message",
  closed_message: "",
  page_url: null,
};
const DEFAULT_NOTIFICATIONS: ShowSettings["notifications"] = { webhook_ids: [], lead_minutes: 60, language: "en", events: {} };

/** A top's settings, then the notifications and the website every show has. */
function SettingsForm({ top }: Readonly<{ top: TopView }>) {
  const canEdit = useCan("operator");
  const station = useStation();
  const [draft, setDraft] = useState<TopSettings>(top.settings);
  useEffect(() => setDraft(top.settings), [top.settings]);
  const categories = useQuery({
    queryKey: ["categories"],
    queryFn: () => unwrap(api.GET("/api/v1/categories")),
  });
  const save = useAction(
    (body: TopSettings) =>
      unwrap(api.PUT("/api/v1/stations/{station_id}/tops/{top_id}", { params: { path: here({ top_id: top.id }) }, body })),
    { success: t("categories.saved"), invalidate: [["tops"]] },
  );
  const remove = useAction(
    () => unwrap(api.DELETE("/api/v1/stations/{station_id}/tops/{top_id}", { params: { path: here({ top_id: top.id }) } })),
    { success: t("tops.deleted"), invalidate: [["tops"]], onSuccess: () => navigate("/shows/top30") },
  );
  const ours = (categories.data ?? []).filter((category) => station !== null && category.station_ids.includes(station));
  const set = (change: Partial<TopSettings>) => setDraft((current) => ({ ...current, ...change }));
  const setSlot = (change: Partial<TopSlot>) =>
    setDraft((current) => ({ ...current, slot: { ...(current.slot ?? DEFAULT_SLOT), ...change } }));
  const dirty = JSON.stringify(draft) !== JSON.stringify(top.settings);
  const slot = draft.slot ?? null;
  const valid = draft.name.trim() !== "" && validTime(draft.reset_time ?? "") && (slot === null || validSlot(slot));
  const plays = draft.plays_percent ?? 0;
  const categorySelect = (value: number | null | undefined, onChange: (id: number | null) => void, name: string) => (
    <select
      className="input"
      aria-label={name}
      value={value ?? ""}
      disabled={!canEdit}
      onChange={(e) => onChange(e.target.value ? Number(e.target.value) : null)}
    >
      <option value="">{t("tops.none")}</option>
      {ours.map((category) => (
        <option key={category.id} value={category.id}>
          {category.name}
        </option>
      ))}
    </select>
  );
  const daySelect = (value: number, onChange: (day: number) => void, name: string) => (
    <select className="input" aria-label={name} value={value} disabled={!canEdit} onChange={(e) => onChange(Number(e.target.value))}>
      {[1, 2, 3, 4, 5, 6, 7].map((day) => (
        <option key={day} value={day}>
          {weekdayName(day)}
        </option>
      ))}
    </select>
  );
  // The settings every show has, as the shows' cards edit them.
  const asShow: ShowSettings = {
    enabled: draft.enabled ?? true,
    hours: [],
    website: draft.website ?? DEFAULT_WEBSITE,
    notifications: draft.notifications ?? DEFAULT_NOTIFICATIONS,
  };
  const setShow: Update = (change) =>
    setDraft((current) => {
      const next = change({
        ...asShow,
        website: current.website ?? DEFAULT_WEBSITE,
        notifications: current.notifications ?? DEFAULT_NOTIFICATIONS,
      });
      return { ...current, website: next.website, notifications: next.notifications };
    });
  const showView: ShowView = {
    module: "top30",
    settings: asShow,
    status: top.status,
    timezone: top.timezone,
    webhooks: top.webhooks,
  };
  const today = new Date().toISOString().slice(0, 10);
  const when = slot ? (slot.every === "month" ? (slot.nth ?? "first") : "week") : "none";
  // Settings saved before (every day, a single day) in the form of days.
  const days = slot ? slotDays(slot) : [];
  const setWhen = (value: string) => {
    if (value === "none") {
      set({ slot: null });
    } else if (value === "week") {
      setSlot({ every: "week", weekdays: slot ? days : [7], weekday: null, nth: null });
    } else {
      setSlot({ every: "month", weekdays: slot ? days : [7], weekday: null, nth: value as Nth });
    }
  };
  const toggleDay = (day: number) => {
    const next = days.includes(day) ? days.filter((other) => other !== day) : [...days, day].sort((a, b) => a - b);
    setSlot({ every: slot?.every === "month" ? "month" : "week", weekdays: next, weekday: null });
  };
  return (
    <>
      <Card title={t("tops.section.top")}>
        <div className="grid-2">
          <Field label={t("tops.name")}>
            <input className="input" value={draft.name} maxLength={60} disabled={!canEdit} onChange={(e) => set({ name: e.target.value })} />
          </Field>
          <label className="check">
            <input
              type="checkbox"
              checked={draft.enabled ?? true}
              disabled={!canEdit}
              onChange={(e) => set({ enabled: e.target.checked })}
            />
            {t("tops.enabled")}
          </label>
        </div>
        <div className="field">
          <span>{t("tops.sizeLabel")}</span>
          <div className="segmented" role="group" aria-label={t("tops.sizeLabel")}>
            {SIZES.map((size) => (
              <button key={size} type="button" aria-pressed={draft.size === size} disabled={!canEdit} onClick={() => set({ size })}>
                {t("tops.size", { size })}
              </button>
            ))}
          </div>
          <span className="hint">{t("tops.sizeHint")}</span>
        </div>
      </Card>

      <Card title={t("tops.section.period")}>
        <fieldset className="stack top-period">
          <legend className="small">{t("tops.periodLegend")}</legend>
          <label className="check">
            <input
              type="radio"
              name="period"
              checked={draft.period === "rolling_7_days"}
              disabled={!canEdit}
              onChange={() => set({ period: "rolling_7_days" })}
            />
            {t("tops.periodRolling")}
          </label>
          <label className="check" style={{ flexWrap: "wrap" }}>
            <input
              type="radio"
              name="period"
              checked={(draft.period ?? "week") === "week"}
              disabled={!canEdit}
              onChange={() => set({ period: "week" })}
            />
            {t("tops.periodWeek")}
            {daySelect(draft.reset_weekday ?? 7, (reset_weekday) => set({ reset_weekday, period: "week" }), t("tops.resetWeekday"))}
            {t("tops.at")}
            <input
              className="input"
              type="time"
              aria-label={t("tops.resetTime")}
              value={draft.reset_time ?? "17:00"}
              disabled={!canEdit}
              onChange={(e) => set({ reset_time: e.target.value })}
            />
          </label>
          <label className="check" style={{ flexWrap: "wrap" }}>
            <input
              type="radio"
              name="period"
              checked={draft.period === "month"}
              disabled={!canEdit}
              onChange={() => set({ period: "month" })}
            />
            {t("tops.periodMonth")}
            <select
              className="input"
              aria-label={t("tops.resetDay")}
              value={draft.reset_day ?? 1}
              disabled={!canEdit}
              onChange={(e) => set({ reset_day: Number(e.target.value), period: "month" })}
            >
              {Array.from({ length: 28 }, (_, index) => index + 1).map((day) => (
                <option key={day} value={day}>
                  {day}
                </option>
              ))}
            </select>
            {t("tops.at")}
            <input
              className="input"
              type="time"
              aria-label={t("tops.resetTime")}
              value={draft.reset_time ?? "00:00"}
              disabled={!canEdit}
              onChange={(e) => set({ reset_time: e.target.value })}
            />
          </label>
        </fieldset>
        <p className="muted small">{t("tops.periodHint")}</p>
      </Card>

      <Card title={t("tops.section.score")}>
        <div className="field">
          <span>{t("tops.ratio")}</span>
          <div className="row top-ratio">
            <span className="small">{t("tops.ratioPlays", { percent: plays })}</span>
            <input
              type="range"
              min={0}
              max={100}
              step={10}
              aria-label={t("tops.ratio")}
              value={100 - plays}
              disabled={!canEdit}
              onChange={(e) => set({ plays_percent: 100 - Number(e.target.value) })}
            />
            <span className="small">{t("tops.ratioLikes", { percent: 100 - plays })}</span>
          </div>
          <code className="top-formula">{t("tops.formula", { plays, likes: 100 - plays })}</code>
          <span className="hint">{t("tops.ratioHint")}</span>
        </div>
        <Field label={t("tops.minVotes")} hint={t("tops.minVotesHint")}>
          <NumberInput
            value={draft.min_votes ?? 10}
            min={0}
            max={1000}
            step={1}
            disabled={!canEdit}
            label={t("tops.minVotes")}
            onChange={(min_votes) => set({ min_votes })}
          />
        </Field>
        <div className="field">
          <span>{t("tops.eligible")}</span>
          <span className="row" style={{ flexWrap: "wrap" }}>
            {ours.map((category) => (
              <label key={category.id} className="check">
                <input
                  type="checkbox"
                  disabled={!canEdit}
                  checked={(draft.category_ids ?? []).includes(category.id)}
                  onChange={(e) =>
                    set({
                      category_ids: e.target.checked
                        ? [...(draft.category_ids ?? []), category.id]
                        : (draft.category_ids ?? []).filter((id) => id !== category.id),
                    })
                  }
                />
                {category.name}
              </label>
            ))}
          </span>
          <span className="hint">{t("tops.eligibleHint")}</span>
        </div>
      </Card>

      <Card title={t("tops.section.show")}>
        <div className="row top-slot" style={{ flexWrap: "wrap" }}>
          <select className="input" aria-label={t("tops.when")} value={when} disabled={!canEdit} onChange={(e) => setWhen(e.target.value)}>
            <option value="none">{t("tops.noShow")}</option>
            <option value="week">{t("tops.every.weekOption")}</option>
            {NTHS.map((nth) => (
              <option key={nth} value={nth}>
                {nthOption(nth)}
              </option>
            ))}
          </select>
          {slot && (
            <div className="segmented" role="group" aria-label={t("tops.days")}>
              {[1, 2, 3, 4, 5, 6, 7].map((day) => (
                <button
                  key={day}
                  type="button"
                  aria-pressed={days.includes(day)}
                  aria-label={weekdayName(day)}
                  title={weekdayName(day)}
                  disabled={!canEdit}
                  onClick={() => toggleDay(day)}
                >
                  {weekdayShort(day)}
                </button>
              ))}
            </div>
          )}
          {slot?.every === "month" && <span>{t("tops.ofMonth")}</span>}
          {slot && (
            <>
              <span>{t("tops.from")}</span>
              <input
                className="input"
                type="time"
                aria-label={t("tops.start")}
                value={slot.start}
                disabled={!canEdit}
                onChange={(e) => setSlot({ start: e.target.value })}
              />
              <span>{t("tops.to")}</span>
              <input
                className="input"
                aria-label={t("tops.end")}
                value={slot.end}
                maxLength={5}
                placeholder="20:00"
                disabled={!canEdit}
                onChange={(e) => setSlot({ end: e.target.value })}
              />
            </>
          )}
        </div>
        {slot && !validSlot(slot) && <span className="badge bad">{t("tops.invalidSlot")}</span>}
        <p className="muted small">
          {t("tops.showHint")}
          {slot && validSlot(slot) && (
            <>
              {" "}
              {t("tops.nextDates", {
                dates: nextDates(slot, today, 3)
                  .map((date) => fullDate(Date.parse(`${date}T12:00:00Z`)))
                  .join(", "),
              })}
            </>
          )}
        </p>
        {top.gives_way.map((other) => (
          <p key={other.top_id} className="top-warning small">
            {givesWayLabel(other)}
          </p>
        ))}
        <div className="grid-2">
          <Field label={t("tops.jingles")} hint={t("tops.jinglesHint")}>
            {categorySelect(draft.jingle_category_id, (id) => set({ jingle_category_id: id }), t("tops.jingles"))}
          </Field>
          <Field label={t("tops.newReleaseFrom")} hint={t("tops.newReleaseHint")}>
            {categorySelect(draft.new_release_category_id, (id) => set({ new_release_category_id: id }), t("tops.newReleaseFrom"))}
          </Field>
        </div>
        <label className="check">
          <input
            type="checkbox"
            checked={draft.hide_unplayed ?? true}
            disabled={!canEdit}
            onChange={(e) => set({ hide_unplayed: e.target.checked })}
          />
          {t("tops.hideUnplayed")}
        </label>
      </Card>

      <Notifications show={showView} draft={asShow} set={setShow} canEdit={canEdit} title={draft.name} />
      <Website module={top.key} title={draft.name} show={showView} draft={asShow} set={setShow} canEdit={canEdit} station={station} />

      {canEdit && (
        <div className="row">
          <button type="button" className="btn primary" disabled={!dirty || !valid || save.isPending} onClick={() => save.mutate(draft)}>
            {t("common.save")}
          </button>
          {dirty && (
            <button type="button" className="btn" onClick={() => setDraft(top.settings)}>
              {t("track.revert")}
            </button>
          )}
          <ConfirmButton
            question={t("tops.deleteConfirm", { name: top.settings.name })}
            confirm={t("tops.delete")}
            onConfirm={() => remove.mutate(undefined)}
          >
            {t("tops.delete")}
          </ConfirmButton>
        </div>
      )}
    </>
  );
}

/** "No. 7 in Top 30", on a track's page: its rank in each top's live chart. */
export function TopRanks({ trackId }: Readonly<{ trackId: number }>) {
  const on = useModule("top30");
  const station = useStation();
  const tops = useQuery({
    queryKey: ["tops", station],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/tops", { params: { path: here() } })),
    enabled: on && station !== null,
  });
  if (!on) {
    return null;
  }
  return (
    <>
      {(tops.data ?? [])
        .filter((top) => !top.error)
        .map((top) => (
          <TopRank key={top.id} top={top} trackId={trackId} />
        ))}
    </>
  );
}

function TopRank({ top, trackId }: Readonly<{ top: TopView; trackId: number }>) {
  const chart = useChart(top.id, null);
  const entry = chart.data?.entries.find((found) => found.track_id === trackId);
  if (!entry) {
    return null;
  }
  return (
    <p className="small">
      <Link to={`/shows/top30/${top.id}`}>{t("tops.rankOf", { rank: entry.rank, name: top.settings.name })}</Link>
    </p>
  );
}
