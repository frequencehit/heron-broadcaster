// SPDX-License-Identifier: AGPL-3.0-or-later

// Listener statistics, from the stream servers: today live, then by day,
// week or month, the hours of the day, the tracks listeners leave and the
// players (see docs/statistics.md).

import { useQuery } from "@tanstack/react-query";
import { type ReactNode, useState } from "react";
import { type Schemas, api, query, unwrap } from "../api";
import { type Bar, BarChart, MinuteChart, type Point } from "../components/ListenerChart";
import { Card, Empty, ErrorView, Loading, PageHead, Segmented } from "../components/ui";
import { type PlainKey, locale, number, t } from "../i18n";
import { duration, shortTime } from "../lib/format";
import {
  type Grouping,
  type Measure,
  addDays,
  barLabel,
  csvUrl,
  dayOf,
  dayStart,
  decimal,
  hourLabel,
  listeningHours,
  measureText,
  percent,
  periodLabel,
  useListenersNow,
} from "../lib/listeners";
import { Link } from "../lib/router";
import { here } from "../lib/station";

type PeriodTotals = Schemas["PeriodTotals"];

const RANGES = [7, 30, 90, 365] as const;

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

function hourTooltip(hour: number, row: Schemas["HourOfDay"] | undefined): ReactNode {
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

export function Statistics() {
  const station = useQuery({
    queryKey: ["settings", "station"],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/settings/station", { params: { path: here() } })),
  });
  const outputs = useQuery({ queryKey: ["outputs"], queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/outputs", { params: { path: here() } })) });
  const [output, setOutput] = useState<number | undefined>(undefined);
  const zone = station.data?.timezone;
  return (
    <>
      <PageHead
        title={t("nav.statistics")}
        description={t("statistics.description", { zone: zone ?? "…" })}
        actions={
          <select
            className="input"
            style={{ width: "auto" }}
            aria-label={t("statistics.output")}
            value={output ?? ""}
            onChange={(e) => setOutput(e.target.value === "" ? undefined : Number(e.target.value))}
          >
            <option value="">{t("statistics.wholeStation")}</option>
            {outputs.data?.map((o) => (
              <option key={o.id} value={o.id}>
                {o.name}
              </option>
            ))}
          </select>
        }
      />
      {station.isPending ? (
        <Loading />
      ) : (
        <div className="stack">
          <Today zone={zone} output={output} />
          <OverTime zone={zone} output={output} />
          <HoursOfDay zone={zone} output={output} />
          <Departures zone={zone} />
          <Players zone={zone} output={output} />
        </div>
      )}
    </>
  );
}

function Tile({ label, value, hint }: Readonly<{ label: string; value: ReactNode; hint?: string | undefined }>) {
  return (
    <div className="stat" title={hint}>
      <span className="value">{value}</span>
      <span className="label">{label}</span>
    </div>
  );
}

function Today({ zone, output }: Readonly<{ zone: string | undefined; output: number | undefined }>) {
  const now = useListenersNow();
  const today = dayOf(Date.now(), zone);
  const from = dayStart(today, zone);
  const to = dayStart(addDays(today, 1), zone);
  const samples = useQuery({
    queryKey: ["listeners", "samples", from, output],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/stations/{station_id}/listeners/samples", {
          params: { path: here(), query: { from_unix_ms: from, to_unix_ms: to, ...query({ output_id: output }) } },
        }),
      ),
    refetchInterval: 60_000,
  });
  const totals = useQuery({
    queryKey: ["listeners", "days", today, today, output, "day"],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/stations/{station_id}/listeners/days", { params: { path: here(), query: { from: today, to: today, ...query({ output_id: output }) } } }),
      ),
    refetchInterval: 60_000,
  });
  const current =
    output === undefined ? now.data?.total : now.data?.outputs.find((o) => o.output_id === output)?.listeners;
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
        {day && day.distinct_listeners == null && <p className="muted small">{t("statistics.noSessionsHelp")}</p>}
      </div>
    </Card>
  );
}

function RangeSelect({ value, onChange }: Readonly<{ value: number; onChange: (days: number) => void }>) {
  return (
    <select className="input" style={{ width: "auto" }} aria-label={t("statistics.range")} value={value} onChange={(e) => onChange(Number(e.target.value))}>
      {RANGES.map((days) => (
        <option key={days} value={days}>
          {t("statistics.lastDays", { count: days })}
        </option>
      ))}
    </select>
  );
}


function OverTime({ zone, output }: Readonly<{ zone: string | undefined; output: number | undefined }>) {
  const [grouping, setGrouping] = useState<Grouping>("day");
  const [range, setRange] = useState(30);
  const [measure, setMeasure] = useState<Measure>("listening_hours");
  const to = dayOf(Date.now(), zone);
  const from = addDays(to, 1 - range);
  const totals = useQuery({
    queryKey: ["listeners", "days", from, to, output, grouping],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/stations/{station_id}/listeners/days", {
          params: { path: here(), query: { from, to, ...query({ output_id: output, group: grouping }) } },
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
      actions={<Csv href={csvUrl("days", { from, to, output_id: output, group: grouping })} />}
    >
      <div className="stack">
        <div className="row">
          <RangeSelect value={range} onChange={setRange} />
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

function HoursOfDay({ zone, output }: Readonly<{ zone: string | undefined; output: number | undefined }>) {
  const [range, setRange] = useState(30);
  const to = dayOf(Date.now(), zone);
  const from = addDays(to, 1 - range);
  const byHour = useQuery({
    queryKey: ["listeners", "hours", from, to, output],
    queryFn: () =>
      unwrap(api.GET("/api/v1/stations/{station_id}/listeners/hours-of-day", { params: { path: here(), query: { from, to, ...query({ output_id: output }) } } })),
  });
  const bars: Bar[] = Array.from({ length: 24 }, (_, hour) => {
    const row = byHour.data?.find((h) => h.hour === hour);
    return {
      key: String(hour),
      label: hour % 3 === 0 ? hourLabel(hour) : "",
      value: row?.average ?? 0,
      tooltip: hourTooltip(hour, row),
    };
  });
  return (
    <Card
      title={t("statistics.hoursOfDay")}
      actions={
        <>
          <RangeSelect value={range} onChange={setRange} />
          <Csv href={csvUrl("hours-of-day", { from, to, output_id: output })} />
        </>
      }
    >
      {byHour.isPending && <Loading />}
      {byHour.isError && <ErrorView error={byHour.error} />}
      {byHour.data &&
        (byHour.data.length === 0 ? (
          <Empty>{t("statistics.noData")}</Empty>
        ) : (
          <>
            <BarChart bars={bars} label={t("statistics.hoursOfDayChart")} />
            <p className="muted small" style={{ marginBottom: 0 }}>
              {t("statistics.hoursOfDayHelp")}
            </p>
          </>
        ))}
    </Card>
  );
}

function Departures({ zone }: Readonly<{ zone: string | undefined }>) {
  const [range, setRange] = useState(30);
  const to = dayOf(Date.now(), zone);
  const from = addDays(to, 1 - range);
  const tracks = useQuery({
    queryKey: ["listeners", "tracks", from, to],
    queryFn: () => unwrap(api.GET("/api/v1/stations/{station_id}/listeners/tracks", { params: { path: here(), query: { from, to, limit: 20 } } })),
  });
  return (
    <Card
      title={t("statistics.departures")}
      actions={
        <>
          <RangeSelect value={range} onChange={setRange} />
          <Csv href={csvUrl("tracks", { from, to, limit: 500 })} />
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

function Players({ zone, output }: Readonly<{ zone: string | undefined; output: number | undefined }>) {
  const [range, setRange] = useState(30);
  const to = dayOf(Date.now(), zone);
  const from = addDays(to, 1 - range);
  const players = useQuery({
    queryKey: ["listeners", "players", from, to, output],
    queryFn: () =>
      unwrap(api.GET("/api/v1/stations/{station_id}/listeners/players", { params: { path: here(), query: { from, to, ...query({ output_id: output }) } } })),
  });
  return (
    <Card title={t("statistics.players")} actions={<RangeSelect value={range} onChange={setRange} />} flush>
      {players.isPending && <Loading />}
      {players.isError && <ErrorView error={players.error} />}
      {players.data?.length === 0 && <Empty>{t("statistics.noSessions")}</Empty>}
      {!!players.data?.length && (
        <div className="table-wrap">
          <table className="table">
            <thead>
              <tr>
                <th>{t("statistics.player")}</th>
                <th className="num">{t("statistics.sessions")}</th>
                <th className="num">{t("statistics.listeningHours")}</th>
              </tr>
            </thead>
            <tbody>
              {players.data.map((player) => (
                <tr key={player.user_agent}>
                  <td className="small mono" style={{ overflowWrap: "anywhere" }}>
                    {player.user_agent || <span className="muted">{t("statistics.unknownPlayer")}</span>}
                  </td>
                  <td className="num">{number(player.sessions)}</td>
                  <td className="num">{listeningHours(player.listening_hours)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </Card>
  );
}
