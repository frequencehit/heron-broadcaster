// SPDX-License-Identifier: AGPL-3.0-or-later

// Listener statistics, from the stream servers, as an audience report for
// the output, server and period of the page's filter: the audience
// figures, the trend, the hours of the week as a heatmap, countries,
// devices and players, the tracks listeners leave, then today live (see
// docs/statistics.md).

import { useQuery } from "@tanstack/react-query";
import { Fragment, type ReactNode, useState } from "react";
import { type Schemas, api, query, unwrap } from "../api";
import { type Bar, BarChart, MinuteChart, type Point } from "../components/ListenerChart";
import { InfoTip } from "../components/InfoTip";
import { Card, Empty, ErrorView, Loading, PageHead, Segmented } from "../components/ui";
import { type PlainKey, locale, number, t } from "../i18n";
import { duration, shortTime } from "../lib/format";
import {
  DEFAULT_DAYS,
  type Filter,
  PERIODS,
  type Period,
  type Grouping,
  type Measure,
  addDays,
  barLabel,
  change,
  changeText,
  countryName,
  csvUrl,
  dayOf,
  dayStart,
  decimal,
  filterOf,
  filterParams,
  heat,
  hourLabel,
  isDbIp,
  lengthShares,
  listeningHours,
  measureText,
  percent,
  periodLabel,
  periodOf,
  shortDay,
  useListenersNow,
  weekHours,
  weekdayNames,
} from "../lib/listeners";
import { Link, setSearch, useLocation } from "../lib/router";
import { here } from "../lib/station";

type PeriodTotals = Schemas["PeriodTotals"];
type Share = Schemas["Share"];
type By = "country" | "device" | "player";
type Audience = Schemas["Audience"];
type HourMeasure = "average" | "tune_in" | "tune_out";

const LENGTHS: { key: keyof Schemas["SessionLengths"]; label: PlainKey }[] = [
  { key: "under_1_min", label: "statistics.lengthUnder1Min" },
  { key: "from_1_to_5_min", label: "statistics.length1To5Min" },
  { key: "from_5_to_30_min", label: "statistics.length5To30Min" },
  { key: "from_30_min_to_2_h", label: "statistics.length30MinTo2H" },
  { key: "over_2_h", label: "statistics.lengthOver2H" },
];


const MEASURES: { value: Measure; label: PlainKey }[] = [
  { value: "listening_hours", label: "statistics.listeningHours" },
  { value: "peak", label: "statistics.peak" },
  { value: "average", label: "statistics.average" },
  { value: "distinct_listeners", label: "statistics.listeners" },
];

/** What the pointer shows on today's chart. */
function minuteTooltip(point: Point): ReactNode {
  return (
    <>
      <strong>{number(point.value)}</strong> · {shortTime(point.at)}
    </>
  );
}

function periodTooltip(row: PeriodTotals, measure: Measure, grouping: Grouping): ReactNode {
  return (
    <>
      <strong>{measureText(row, measure)}</strong> · {periodLabel(row.day, grouping)}
    </>
  );
}

function hourTooltip(hour: number, row: Schemas["HourOfDay"] | undefined, measure: HourMeasure): ReactNode {
  if (row && measure !== "average") {
    const value = measure === "tune_in" ? row.tune_in : row.tune_out;
    return (
      <>
        <strong>{t("statistics.perDay", { value: decimal(value) })}</strong> · {hourLabel(hour)}
      </>
    );
  }
  if (!row) {
    return (
      <>
        {hourLabel(hour)} · {t("statistics.notMeasured")}
      </>
    );
  }
  return (
    <>
      <strong>{decimal(row.average)}</strong> {t("statistics.onAverage")} · {hourLabel(hour)}
      <br />
      {t("statistics.peak")} {number(row.peak)} · {t("statistics.listeningHours")} {listeningHours(row.listening_hours)}
    </>
  );
}

function Csv({ href }: Readonly<{ href: string }>) {
  return (
    <a className="btn small" href={href} download>
      {t("statistics.csv")}
    </a>
  );
}

/** The servers of the filter's output (every server read, for the whole station). */
function serversOf(now: Schemas["ListenersNow"] | undefined, output: number | undefined): Schemas["ServerListeners"][] {
  const parts = output === undefined ? undefined : now?.outputs.find((o) => o.output_id === output)?.servers;
  return (now?.servers ?? []).filter((s) => !parts || parts.some((p) => p.server_id === s.server_id));
}

export function Statistics() {
  const station = useQuery({
    queryKey: ["settings", "station"],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/settings/station", { params: { path: here() } })),
  });
  const outputs = useQuery({ queryKey: ["outputs"], queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/outputs", { params: { path: here() } })) });
  const now = useListenersNow();
  // The output and server are in the address, for the whole page.
  const filter = filterOf(useLocation().search);
  const servers = serversOf(now.data, filter.output);
  const zone = station.data?.timezone;
  const period = periodOf(useLocation().search, dayOf(Date.now(), zone));
  const pick = (value: string) => (value === "" ? null : Number(value));
  return (
    <>
      <PageHead
        title={t("nav.statistics")}
        description={t("statistics.description", { zone: zone ?? "…" })}
        actions={
          <>
            <select
              className="input"
              style={{ width: "auto" }}
              aria-label={t("statistics.output")}
              value={filter.output ?? ""}
              onChange={(e) => setSearch({ output: pick(e.target.value) })}
            >
              <option value="">{t("statistics.wholeStation")}</option>
              {outputs.data?.map((o) => (
                <option key={o.id} value={o.id}>
                  {o.name}
                </option>
              ))}
            </select>
            {(servers.length > 1 || filter.server !== undefined) && (
              <select
                className="input"
                style={{ width: "auto" }}
                aria-label={t("statistics.server")}
                value={filter.server ?? ""}
                onChange={(e) => setSearch({ server: pick(e.target.value) })}
              >
                <option value="">{t("statistics.allServers")}</option>
                {servers.map((s) => (
                  <option key={s.server_id} value={s.server_id}>
                    {s.relay_of == null ? s.name : `${s.name} (${t("statistics.relay")})`}
                  </option>
                ))}
                {filter.server !== undefined && !servers.some((s) => s.server_id === filter.server) && (
                  <option value={filter.server}>#{filter.server}</option>
                )}
              </select>
            )}
            <RangeSelect value={period.days} onChange={(days) => setSearch({ days: days === DEFAULT_DAYS ? null : days })} />
          </>
        }
      />
      {station.isPending ? (
        <Loading />
      ) : (
        <div className="stack">
          <Today zone={zone} filter={filter} />
          <AudienceCard zone={zone} filter={filter} period={period} />
          <OverTime filter={filter} period={period} />
          <HoursOfDay filter={filter} period={period} />
          <Countries filter={filter} period={period} />
          <DevicesAndPlayers filter={filter} period={period} />
          <Departures filter={filter} period={period} />
          <HowCounted zone={zone} />
        </div>
      )}
    </>
  );
}

/** How the figures are counted, at the bottom of the page. */
function HowCounted({ zone }: Readonly<{ zone: string | undefined }>) {
  return (
    <details className="stats-how">
      <summary>{t("statistics.howCounted")}</summary>
      <ul>
        <li>{t("statistics.howShort")}</li>
        <li>{t("statistics.howListener")}</li>
        <li>{t("statistics.howRelays")}</li>
        <li>{t("statistics.howQuarters", { zone: zone ?? "…" })}</li>
        <li>{t("statistics.howPrevious")}</li>
        <li>{t("statistics.tuneTip")}</li>
        <li>{t("statistics.howPeak")}</li>
        <li>{t("statistics.howCountries")}</li>
        <li>{t("statistics.howOther")}</li>
      </ul>
    </details>
  );
}

function Tile({
  label,
  value,
  hint,
  tip,
  sub,
  delta,
}: Readonly<{
  label: string;
  value: ReactNode;
  hint?: string | undefined;
  /** What the figure is: one sentence, behind an info button. */
  tip?: string;
  sub?: ReactNode;
  delta?: { ratio: number | null; previous: string } | undefined;
}>) {
  return (
    <div className="stat" title={hint}>
      <span className="value">{value}</span>
      <span className="label">
        {label}
        {tip && (
          <>
            {" "}
            <InfoTip label={t("statistics.about", { name: label })}>{tip}</InfoTip>
          </>
        )}
      </span>
      {sub && <span className="muted small">{sub}</span>}
      {delta && (
        <span className={`small ${delta.ratio !== null && delta.ratio < 0 ? "warn" : "muted"}`} title={t("statistics.previousValue", { value: delta.previous })}>
          {t("statistics.vsPrevious", { change: changeText(delta.ratio, locale()) })}
        </span>
      )}
    </div>
  );
}

/** The day and time of a peak, in station time. */
function peakAt(unixMs: number | null | undefined, zone: string | undefined): string | undefined {
  if (unixMs === null || unixMs === undefined) {
    return undefined;
  }
  return t("statistics.peakAt", { day: shortDay(dayOf(unixMs, zone)), time: shortTime(unixMs) });
}

function AudienceCard({ zone, filter, period }: Readonly<{ zone: string | undefined; filter: Filter; period: Period }>) {
  const { from, to } = period;
  const figures = useQuery({
    queryKey: ["listeners", "audience", from, to, filter],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/stations/{station_id}/listeners/audience", {
          params: { path: here(), query: { from, to, ...query(filterParams(filter)) } },
        }),
      ),
  });
  const current: Audience | undefined = figures.data?.current;
  const previous: Audience | undefined = figures.data?.previous;
  const delta = (pick: (a: Audience) => number | null | undefined, write: (value: number | null | undefined) => string) =>
    current && previous ? { ratio: change(pick(current), pick(previous)), previous: write(pick(previous)) } : undefined;
  const hours = (value: number | null | undefined) => listeningHours(value);
  const seconds = (value: number | null | undefined) => (value == null ? "–" : duration(value * 1000));
  const count = (value: number | null | undefined) => (value == null ? "–" : number(value));
  const shares = lengthShares(current?.session_lengths);
  const lengthBars: Bar[] = shares.map((s, i) => ({
    key: s.key,
    label: t(LENGTHS[i]?.label ?? "statistics.lengthOver2H"),
    value: s.share * 100,
    tooltip: (
      <>
        <strong>{t(LENGTHS[i]?.label ?? "statistics.lengthOver2H")}</strong> · {t("statistics.lengthTooltip", { share: percent(s.share, locale()), count: number(s.count) })}
      </>
    ),
  }));

  return (
    <Card
      title={t("statistics.audience")}
      actions={
        <>
          <Csv href={csvUrl("audience", { from, to, ...filterParams(filter) })} />
        </>
      }
    >
      {figures.isPending && <Loading />}
      {figures.isError && <ErrorView error={figures.error} />}
      {current && (
        <div className="stack">
          <div className="stats-tiles">
            <Tile
              label={t("statistics.cume")}
              value={decimal(current.cume, period.days === 1 ? 0 : 1)}
              tip={t("statistics.cumeTip")}
              sub={period.days === 1 || current.listener_days == null ? undefined : t("statistics.cumeHint", { count: number(current.listener_days) })}
              delta={delta((a) => a.cume, (v) => decimal(v))}
            />
            <Tile label={t("statistics.ttsl")} value={`${hours(current.ttsl_hours)} h`} tip={t("statistics.ttslHint")} delta={delta((a) => a.ttsl_hours, hours)} />
            <Tile
              label={t("statistics.atsl")}
              value={seconds(current.atsl_s)}
              tip={t("statistics.atslHint")}
              sub={current.median_session_s == null ? undefined : t("statistics.median", { value: seconds(current.median_session_s) })}
              delta={delta((a) => a.atsl_s, seconds)}
            />
            <Tile label={t("statistics.aqh")} value={decimal(current.aqh)} tip={t("statistics.aqhHint")} delta={delta((a) => a.aqh, (v) => decimal(v))} />
            <Tile
              label={t("statistics.peakAudience")}
              value={count(current.peak)}
              tip={t("statistics.peakTip")}
              sub={peakAt(current.peak_at_unix_ms, zone)}
              delta={delta((a) => a.peak, count)}
            />
            <Tile label={t("statistics.sessions")} value={count(current.sessions)} tip={t("statistics.sessionsTip")} delta={delta((a) => a.sessions, count)} />
          </div>
          {current.sessions == null ? (
            <p className="muted small" style={{ margin: 0 }}>
              {t("statistics.noAudience")}
            </p>
          ) : (
            <>
              <p className="muted small" style={{ margin: 0 }}>
                {t("statistics.audienceHelp")}
              </p>
              {current.sessions > 0 && (
                <>
                  <h3 className="small" style={{ margin: 0 }}>
                    {t("statistics.sessionLengths")} <InfoTip label={t("statistics.about", { name: t("statistics.sessionLengths") })}>{t("statistics.lengthsTip")}</InfoTip>
                  </h3>
                  <BarChart bars={lengthBars} label={t("statistics.sessionLengths")} />
                </>
              )}
            </>
          )}
        </div>
      )}
    </Card>
  );
}

/** The listeners now of a server's part, for one output or all. */
function serverNow(now: Schemas["ListenersNow"] | undefined, server: number, output: number | undefined): number | null | undefined {
  if (output === undefined) {
    return now?.servers.find((s) => s.server_id === server)?.listeners;
  }
  const outputNow = now?.outputs.find((o) => o.output_id === output);
  return outputNow?.servers.find((s) => s.server_id === server)?.listeners;
}

function Today({ zone, filter }: Readonly<{ zone: string | undefined; filter: Filter }>) {
  const now = useListenersNow();
  const { output, server } = filter;
  const today = dayOf(Date.now(), zone);
  const from = dayStart(today, zone);
  const to = dayStart(addDays(today, 1), zone);
  const samples = useQuery({
    queryKey: ["listeners", "samples", from, output, server],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/stations/{station_id}/listeners/samples", {
          params: { path: here(), query: { from_unix_ms: from, to_unix_ms: to, ...query({ output_id: output, server_id: server }) } },
        }),
      ),
    refetchInterval: 60_000,
  });
  const totals = useQuery({
    queryKey: ["listeners", "days", today, today, filter, "day"],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/stations/{station_id}/listeners/days", {
          params: { path: here(), query: { from: today, to: today, ...query(filterParams(filter)) } },
        }),
      ),
    refetchInterval: 60_000,
  });
  const servers = serversOf(now.data, output);
  const current =
    server !== undefined
      ? serverNow(now.data, server, output)
      : output === undefined
        ? now.data?.total
        : now.data?.outputs.find((o) => o.output_id === output)?.listeners;
  const day = totals.data?.[0];
  const unread = now.data?.outputs.filter((o) => o.error && (output === undefined || o.output_id === output)) ?? [];

  return (
    <Card title={t("statistics.today")}>
      <div className="stack">
        {unread.map((o) => (
          <p key={o.output_id} className="warn small" style={{ margin: 0 }}>
            {t("statistics.unread", { name: o.name, error: o.error ?? "" })}
          </p>
        ))}
        <div className="stats-tiles">
          <Tile label={t("statistics.now")} value={current === undefined || current === null ? "–" : number(current)} />
          <Tile label={t("statistics.peak")} value={day ? number(day.peak) : "–"} />
          <Tile label={t("statistics.listeningHours")} value={listeningHours(day?.listening_hours)} />
          <Tile
            label={t("statistics.listeners")}
            value={decimal(day?.distinct_listeners, 0)}
            hint={day && day.distinct_listeners == null ? t("statistics.noSessions") : undefined}
          />
          <Tile
            label={t("statistics.averageSession")}
            value={day?.average_session_s == null ? "–" : duration(day.average_session_s * 1000)}
          />
        </div>
        {samples.isError && <ErrorView error={samples.error} />}
        {samples.data &&
          (samples.data.length === 0 ? (
            <Empty>{t("statistics.noSamples")}</Empty>
          ) : (
            <MinuteChart
              label={t("statistics.todayChart")}
              points={samples.data.map((s) => ({ at: s.at_unix_ms, value: s.listeners }))}
              from={from}
              to={to}
              tickLabel={shortTime}
              tooltip={minuteTooltip}
            />
          ))}
        {servers.length > 1 && server === undefined && (
          <table className="table">
            <caption className="muted small" style={{ textAlign: "left" }}>
              {t("statistics.servers")}
            </caption>
            <tbody>
              {servers.map((s) => {
                const listeners = serverNow(now.data, s.server_id, output);
                return (
                  <tr key={s.server_id}>
                    <td>
                      {s.name}
                      {s.relay_of != null && <span className="muted small"> · {t("statistics.relay")}</span>}
                    </td>
                    <td className="num">
                      {listeners == null ? (
                        <span className="warn small" title={s.error ?? undefined}>
                          {t("statistics.notRead")}
                        </span>
                      ) : (
                        number(listeners)
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
        {day && day.distinct_listeners == null && <p className="muted small">{t("statistics.noSessionsHelp")}</p>}
      </div>
    </Card>
  );
}

function RangeSelect({ value, onChange }: Readonly<{ value: number; onChange: (days: number) => void }>) {
  return (
    <select className="input" style={{ width: "auto" }} aria-label={t("statistics.range")} value={value} onChange={(e) => onChange(Number(e.target.value))}>
      {PERIODS.map((days) => (
        <option key={days} value={days}>
          {t("statistics.lastDays", { count: days })}
        </option>
      ))}
    </select>
  );
}


function OverTime({ filter, period }: Readonly<{ filter: Filter; period: Period }>) {
  const [grouping, setGrouping] = useState<Grouping>("day");
  const [measure, setMeasure] = useState<Measure>("listening_hours");
  const { from, to } = period;
  const totals = useQuery({
    queryKey: ["listeners", "days", from, to, filter, grouping],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/stations/{station_id}/listeners/days", {
          params: { path: here(), query: { from, to, ...query({ ...filterParams(filter), group: grouping }) } },
        }),
      ),
  });
  const rows = totals.data ?? [];
  const bars: Bar[] = rows.map((row) => ({
    key: row.day,
    label: barLabel(row.day, grouping),
    value: row[measure] ?? 0,
    tooltip: periodTooltip(row, measure, grouping),
  }));
  const measureLabel = t(MEASURES.find((m) => m.value === measure)?.label ?? "statistics.listeningHours");

  return (
    <Card
      title={t("statistics.overTime")}
      actions={<Csv href={csvUrl("days", { from, to, ...filterParams(filter), group: grouping })} />}
    >
      <div className="stack">
        <div className="row">
          <Segmented
            label={t("statistics.grouping")}
            value={grouping}
            onChange={setGrouping}
            options={[
              { value: "day", label: t("statistics.byDay") },
              { value: "week", label: t("statistics.byWeek") },
              { value: "month", label: t("statistics.byMonth") },
            ]}
          />
          <Segmented
            label={t("statistics.measure")}
            value={measure}
            onChange={setMeasure}
            options={MEASURES.map((m) => ({ value: m.value, label: t(m.label) }))}
          />
        </div>
        {totals.isPending && <Loading />}
        {totals.isError && <ErrorView error={totals.error} />}
        {totals.data && rows.length === 0 && <Empty>{t("statistics.noData")}</Empty>}
        {rows.length > 0 && (
          <>
            <BarChart bars={bars} label={`${t("statistics.overTime")}: ${measureLabel}`} />
            {grouping !== "day" && measure === "distinct_listeners" && (
              <p className="muted small" style={{ margin: 0 }}>
                {t("statistics.perDayHelp")}
              </p>
            )}
            <div className="table-wrap">
              <table className="table">
                <thead>
                  <tr>
                    <th>{t("statistics.period")}</th>
                    <th className="num">{t("statistics.listeners")}</th>
                    <th className="num">{t("statistics.peak")}</th>
                    <th className="num hide-narrow">{t("statistics.average")}</th>
                    <th className="num">{t("statistics.listeningHours")}</th>
                    <th className="num hide-narrow">{t("statistics.sessions")}</th>
                    <th className="num hide-narrow">{t("statistics.averageSession")}</th>
                  </tr>
                </thead>
                <tbody>
                  {[...rows].reverse().map((row) => (
                    <tr key={row.day}>
                      <td>{periodLabel(row.day, grouping)}</td>
                      <td className="num">{decimal(row.distinct_listeners, grouping === "day" ? 0 : 1)}</td>
                      <td className="num">{number(row.peak)}</td>
                      <td className="num hide-narrow">{decimal(row.average)}</td>
                      <td className="num">{listeningHours(row.listening_hours)}</td>
                      <td className="num hide-narrow">{row.sessions == null ? "–" : number(row.sessions)}</td>
                      <td className="num hide-narrow">
                        {row.average_session_s == null ? "–" : duration(row.average_session_s * 1000)}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </>
        )}
      </div>
    </Card>
  );
}

function HoursOfDay({ filter, period }: Readonly<{ filter: Filter; period: Period }>) {
  const [measure, setMeasure] = useState<HourMeasure>("average");
  const { from, to } = period;
  const byHour = useQuery({
    queryKey: ["listeners", "hours", from, to, filter],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/stations/{station_id}/listeners/hours-of-day", { params: { path: here(), query: { from, to, ...query(filterParams(filter)) } } }),
      ),
  });
  const hourly = useQuery({
    queryKey: ["listeners", "days", from, to, filter, "hour"],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/stations/{station_id}/listeners/days", {
          params: { path: here(), query: { from, to, ...query({ ...filterParams(filter), group: "hour" as const }) } },
        }),
      ),
  });
  const tunes = byHour.data?.some((h) => h.tune_in != null) ?? false;
  const bars: Bar[] = Array.from({ length: 24 }, (_, hour) => {
    const row = byHour.data?.find((h) => h.hour === hour);
    return {
      key: String(hour),
      label: hour % 3 === 0 ? hourLabel(hour) : "",
      value: (measure === "average" ? row?.average : measure === "tune_in" ? row?.tune_in : row?.tune_out) ?? 0,
      tooltip: hourTooltip(hour, row, measure),
    };
  });
  return (
    <Card
      title={t("statistics.hoursOfDay")}
      actions={
        <>
          <Csv href={csvUrl("hours-of-day", { from, to, ...filterParams(filter) })} />
        </>
      }
    >
      {byHour.isPending && <Loading />}
      {byHour.isError && <ErrorView error={byHour.error} />}
      {byHour.data &&
        (byHour.data.length === 0 ? (
          <Empty>{t("statistics.noData")}</Empty>
        ) : (
          <div className="stack">
            {hourly.data && <Heatmap cells={weekHours(hourly.data)} />}
            {tunes && (
              <div className="row">
              <Segmented
                label={t("statistics.measure")}
                value={measure}
                onChange={setMeasure}
                options={[
                  { value: "average", label: t("statistics.listeners") },
                  { value: "tune_in", label: t("statistics.tuneIn") },
                  { value: "tune_out", label: t("statistics.tuneOut") },
                ]}
              />
              <InfoTip label={t("statistics.about", { name: `${t("statistics.tuneIn")} / ${t("statistics.tuneOut")}` })}>{t("statistics.tuneTip")}</InfoTip>
              </div>
            )}
            <BarChart bars={bars} label={t("statistics.hoursOfDayChart")} />
            <p className="muted small" style={{ margin: 0 }}>
              {measure === "average" ? t("statistics.heatmapHelp") : t("statistics.tuneHelp")}
            </p>
          </div>
        ))}
    </Card>
  );
}

function Departures({ filter, period }: Readonly<{ filter: Filter; period: Period }>) {
  const { from, to } = period;
  const tracks = useQuery({
    queryKey: ["listeners", "tracks", from, to, filter],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/stations/{station_id}/listeners/tracks", { params: { path: here(), query: { from, to, limit: 20, ...query(filterParams(filter)) } } }),
      ),
  });
  return (
    <Card
      title={t("statistics.departures")}
      actions={
        <>
          <Csv href={csvUrl("tracks", { from, to, limit: 500, ...filterParams(filter) })} />
        </>
      }
      flush
    >
      <p className="muted small" style={{ margin: "0.75rem 1rem" }}>
        {t("statistics.departuresHelp")}
      </p>
      {tracks.isPending && <Loading />}
      {tracks.isError && <ErrorView error={tracks.error} />}
      {tracks.data?.length === 0 && <Empty>{t("statistics.noTracks")}</Empty>}
      {!!tracks.data?.length && (
        <div className="table-wrap">
          <table className="table">
            <thead>
              <tr>
                <th>{t("history.track")}</th>
                <th className="num">{t("statistics.plays")}</th>
                <th className="num hide-narrow">{t("statistics.atStart")}</th>
                <th className="num hide-narrow">{t("statistics.atEnd")}</th>
                <th className="num">{t("statistics.departed")}</th>
                <th className="num">{t("statistics.rate")}</th>
              </tr>
            </thead>
            <tbody>
              {tracks.data.map((track) => (
                <tr key={track.track_id}>
                  <td className="title-cell ellipsis">
                    <Link to={`/library/${track.track_id}`}>
                      {[track.artist, track.title].filter(Boolean).join(" – ") || t("history.unknownTrack")}
                    </Link>
                  </td>
                  <td className="num">{number(track.plays)}</td>
                  <td className="num hide-narrow">{decimal(track.listeners_start)}</td>
                  <td className="num hide-narrow">{decimal(track.listeners_end)}</td>
                  <td className="num">{track.departures == null ? "–" : number(track.departures)}</td>
                  <td className="num">{percent(track.departure_rate, locale())}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </Card>
  );
}

/** Average listeners by day of the week (rows, Monday first) and hour. */
function Heatmap({ cells }: Readonly<{ cells: (number | null)[][] }>) {
  const top = Math.max(0, ...cells.flat().map((v) => v ?? 0));
  const days = weekdayNames(locale());
  return (
    <div className="heatmap" role="img" aria-label={t("statistics.heatmap")}>
      <span />
      {Array.from({ length: 24 }, (_, hour) => (
        <span key={hour} className="heatmap-hour">
          {hour % 3 === 0 ? hourLabel(hour) : ""}
        </span>
      ))}
      {cells.map((row, day) => (
        <Fragment key={days[day]}>
          <span className="heatmap-day">{days[day]}</span>
          {row.map((value, hour) => (
            <span
              key={hour}
              className={value === null ? "heatmap-cell empty" : "heatmap-cell"}
              style={value === null ? undefined : { background: `color-mix(in srgb, var(--accent) ${Math.round(heat(value, top) * 100)}%, var(--surface-2))` }}
              title={t("statistics.heatmapCell", { day: days[day] ?? "", hour: hourLabel(hour), value: value === null ? t("statistics.notMeasured") : decimal(value) })}
            />
          ))}
        </Fragment>
      ))}
    </div>
  );
}

const DEVICES: Record<string, PlainKey> = {
  mobile: "statistics.deviceMobile",
  desktop: "statistics.deviceDesktop",
  smart_speaker: "statistics.deviceSmartSpeaker",
  car: "statistics.deviceCar",
  tv: "statistics.deviceTv",
};

/** A group's name: a country, a device or a player family. */
function shareName(share: Share, by: By): string {
  if (share.kind === "unknown") {
    return t("statistics.unknownCountry");
  }
  if (share.kind === "other" || share.name == null) {
    return t("statistics.otherGroup");
  }
  if (by === "country") {
    return countryName(share.name, locale());
  }
  if (by === "device") {
    const key = DEVICES[share.name];
    return key ? t(key) : share.name;
  }
  return share.name;
}

function useBreakdown(by: By, filter: Filter, period: Period) {
  const { from, to } = period;
  return useQuery({
    queryKey: ["listeners", "breakdown", by, from, to, filter],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/stations/{station_id}/listeners/breakdown", {
          params: { path: here(), query: { from, to, by, ...query(filterParams(filter)) } },
        }),
      ),
  });
}

/** The groups of a breakdown as a table with bars: share of listeners and of TTSL. */
function ShareTable({ by, groups, caption }: Readonly<{ by: By; groups: Share[]; caption: string }>) {
  return (
    <div className="table-wrap">
      <table className="table shares">
        <caption className="muted small" style={{ textAlign: "left" }}>
          {caption}
        </caption>
        <thead>
          <tr>
            <th>{caption}</th>
            <th className="num">{t("statistics.listenerShare")}</th>
            <th className="num">{t("statistics.ttslShare")}</th>
            <th className="num hide-narrow">{t("statistics.sessions")}</th>
          </tr>
        </thead>
        <tbody>
          {groups.map((share) => {
            const name = shareName(share, by);
            return (
              <tr key={`${share.kind}-${share.name ?? ""}`}>
                <td>
                  <span className={share.kind === "named" ? undefined : "muted"}>{name}</span>
                  <span className="share-bar" aria-hidden="true">
                    <span style={{ width: `${Math.round(share.listener_share * 100)}%` }} />
                  </span>
                </td>
                <td className="num">{percent(share.listener_share, locale())}</td>
                <td className="num">{percent(share.ttsl_share, locale())}</td>
                <td className="num hide-narrow">{number(share.sessions)}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

function SharesHelp() {
  return (
    <p className="muted small" style={{ margin: 0 }}>
      {t("statistics.sharesHelp")} <InfoTip label={t("statistics.about", { name: t("statistics.otherGroup") })}>{t("statistics.sharesTip")}</InfoTip>
    </p>
  );
}

function Countries({ filter, period }: Readonly<{ filter: Filter; period: Period }>) {
  const countries = useBreakdown("country", filter, period);
  const data = countries.data;
  // Off without a GeoIP database, unless sessions kept a country from before.
  if (data && data.geoip == null && !data.groups.some((g) => g.kind === "named")) {
    return null;
  }
  return (
    <Card
      title={t("statistics.countries")}
      actions={<Csv href={csvUrl("breakdown", { from: period.from, to: period.to, by: "country", ...filterParams(filter) })} />}
    >
      {countries.isPending && <Loading />}
      {countries.isError && <ErrorView error={countries.error} />}
      {data &&
        (data.groups.length === 0 ? (
          <Empty>{t("statistics.noShares")}</Empty>
        ) : (
          <div className="stack">
            <ShareTable by="country" groups={data.groups} caption={t("statistics.country")} />
            <SharesHelp />
          </div>
        ))}
      {data?.geoip && (
        <p className="muted small" style={{ marginBottom: 0 }}>
          {isDbIp(data.geoip) ? (
            <a href="https://db-ip.com" target="_blank" rel="noreferrer">
              {t("statistics.dbipCredit")}
            </a>
          ) : (
            t("statistics.geoipFrom", { database: data.geoip })
          )}
        </p>
      )}
    </Card>
  );
}

function DevicesAndPlayers({ filter, period }: Readonly<{ filter: Filter; period: Period }>) {
  const devices = useBreakdown("device", filter, period);
  const players = useBreakdown("player", filter, period);
  const csv = (by: By) => csvUrl("breakdown", { from: period.from, to: period.to, by, ...filterParams(filter) });
  const empty = devices.data?.groups.length === 0 && players.data?.groups.length === 0;
  return (
    <Card
      title={t("statistics.devicesPlayers")}
      actions={
        <>
          <a className="btn small" href={csv("device")} download>
            {t("statistics.csvDevices")}
          </a>
          <a className="btn small" href={csv("player")} download>
            {t("statistics.csvPlayers")}
          </a>
        </>
      }
    >
      {(devices.isPending || players.isPending) && <Loading />}
      {devices.isError && <ErrorView error={devices.error} />}
      {empty && <Empty>{t("statistics.noShares")}</Empty>}
      {!empty && devices.data && players.data && (
        <div className="stack">
          <div className="shares-grid">
            <ShareTable by="device" groups={devices.data.groups} caption={t("statistics.device")} />
            <ShareTable by="player" groups={players.data.groups} caption={t("statistics.player")} />
          </div>
          <SharesHelp />
        </div>
      )}
    </Card>
  );
}
