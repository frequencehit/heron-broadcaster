// SPDX-License-Identifier: AGPL-3.0-or-later

// The Statistics page comparing stations (docs/statistics.md#web-ui): the
// station choice, then today, the audience figures, the trend, the hours of
// the day and the shares, one color per station. Two stations: tiles side
// by side with the difference; all of them: a table per station. The
// figures come from the routes of several stations, among those the user
// sees.

import { useQuery } from "@tanstack/react-query";
import { type CSSProperties, type ReactNode, useState } from "react";
import { type Schemas, api, unwrap } from "../api";
import { InfoTip } from "../components/InfoTip";
import { Legend, LinesChart, type Series } from "../components/ListenerChart";
import { Card, Empty, ErrorView, Loading, Segmented } from "../components/ui";
import { type PlainKey, locale, number, t } from "../i18n";
import { timeTicks } from "../lib/chart";
import { duration, shortTime } from "../lib/format";
import {
  type Grouping,
  type Measure,
  type Period,
  type StationsView,
  addDays,
  barLabel,
  change,
  changeText,
  dayOf,
  dayStart,
  decimal,
  hourLabel,
  listeningHours,
  measureText,
  percent,
  periodLabel,
  seriesColor,
  shareColumns,
  stationsCsvUrl,
  stationsParam,
} from "../lib/listeners";
import { useModule } from "../lib/modules";
import { type StationInfo, here } from "../lib/station";
import { type By, Csv, GeoipCredit, MEASURES, SharesHelp, peakAt, shareName } from "./Statistics";

type Audience = Schemas["Audience"];
type StationRef = Schemas["StationRef"];
type Compared = Exclude<StationsView, { kind: "one" } | { kind: "all" }>;

/** The station choice of the page: this station, all of them added up, or a comparison. */
export function StationsSelect({
  stations,
  view,
  onChange,
}: Readonly<{ stations: StationInfo[]; view: StationsView; onChange: (view: StationsView) => void }>) {
  const current = here().station_id;
  const value = view.kind === "compare" ? view.ids.join(",") : view.kind === "one" ? "" : view.kind;
  const options = stations.filter((s) => s.id !== current).map((s) => ({ value: `${current},${s.id}`, name: s.name }));
  // A comparison bookmarked from another station.
  const other =
    view.kind === "compare" && !options.some((o) => o.value === value)
      ? stations
          .filter((s) => view.ids.includes(s.id))
          .map((s) => s.name)
          .join(", ")
      : null;
  const choose = (choice: string) => {
    if (choice === "all" || choice === "compare-all") {
      onChange({ kind: choice });
    } else if (choice === "") {
      onChange({ kind: "one" });
    } else {
      onChange({ kind: "compare", ids: choice.split(",").map(Number) });
    }
  };
  return (
    <select className="input" style={{ width: "auto" }} aria-label={t("statistics.stationsView")} value={value} onChange={(e) => choose(e.target.value)}>
      <option value="">{t("statistics.thisStation")}</option>
      <option value="all">{t("statistics.allStations")}</option>
      {options.map((o) => (
        <option key={o.value} value={o.value}>
          {t("statistics.compareWith", { name: o.name })}
        </option>
      ))}
      {other !== null && <option value={value}>{t("statistics.compareWith", { name: other })}</option>}
      {stations.length > 2 && <option value="compare-all">{t("statistics.compareAll")}</option>}
    </select>
  );
}

/** Each station's listeners now, and their total; read again every half minute. */
export function useStationsNow(view: StationsView, enabled = true) {
  const on = useModule("listener_statistics");
  const stations = stationsParam(view);
  return useQuery({
    queryKey: ["listeners", "stations", "now", stations],
    queryFn: () => unwrap(api.GET("/api/v1/stations/listeners/now", { params: { query: { stations } } })),
    refetchInterval: 30_000,
    enabled: on && enabled,
  });
}

function useStationsDays(view: StationsView, from: string, to: string, group: Grouping | "hour", refetch?: number) {
  const stations = stationsParam(view);
  return useQuery({
    queryKey: ["listeners", "stations", "days", stations, from, to, group],
    queryFn: () => unwrap(api.GET("/api/v1/stations/listeners/days", { params: { query: { stations, from, to, group } } })),
    refetchInterval: refetch ?? false,
  });
}

/** The stations of an answer as chart series, one color each. */
function series<T extends StationRef>(stations: T[], points: (station: T) => Series["points"]): Series[] {
  return stations.map((s, i) => ({ key: String(s.station_id), label: s.name, color: seriesColor(i), points: points(s) }));
}

function legend(stations: StationRef[]) {
  return <Legend series={stations.map((s, i) => ({ key: String(s.station_id), label: s.name, color: seriesColor(i) }))} />;
}

/** The values of a tooltip of several lines: each station's. */
function lineValues(values: { series: Series; value: number | undefined }[], write: (value: number) => string): ReactNode {
  return values.map(({ series: s, value }) => (
    <span key={s.key} style={{ display: "block" }}>
      <span className="chart-swatch" style={{ background: s.color }} aria-hidden="true" /> {s.label} <strong>{value === undefined ? "–" : write(value)}</strong>
    </span>
  ));
}

/**
 * A figure of each station side by side, each with its color; with two
 * stations, the second's difference against the first.
 */
function ComparedTile({
  label,
  tip,
  values,
}: Readonly<{
  label: string;
  tip?: string;
  values: { station: StationRef; color: string; value: number | null | undefined; text: string }[];
}>) {
  const [first, second] = values;
  return (
    <div className="stat">
      <span className="compared">
        {values.map((v) => (
          <span key={v.station.station_id} className="value" style={{ "--station-color": v.color } as CSSProperties} title={v.station.name}>
            <span className="visually-hidden">{v.station.name}: </span>
            {v.text}
          </span>
        ))}
      </span>
      <span className="label">
        {label}
        {tip && (
          <>
            {" "}
            <InfoTip label={t("statistics.about", { name: label })}>{tip}</InfoTip>
          </>
        )}
      </span>
      {values.length === 2 && first && second && (
        <span className="muted small">
          {t("statistics.versus", { name: second.station.name, other: first.station.name, change: changeText(change(second.value, first.value), locale()) })}
        </span>
      )}
    </div>
  );
}

export function CompareToday({ view, zone }: Readonly<{ view: Compared; zone: string | undefined }>) {
  const stations = stationsParam(view);
  const now = useStationsNow(view);
  const today = dayOf(Date.now(), zone);
  const from = dayStart(today, zone);
  const to = dayStart(addDays(today, 1), zone);
  const samples = useQuery({
    queryKey: ["listeners", "stations", "samples", stations, from],
    queryFn: () => unwrap(api.GET("/api/v1/stations/listeners/samples", { params: { query: { stations, from_unix_ms: from, to_unix_ms: to } } })),
    refetchInterval: 60_000,
  });
  const totals = useStationsDays(view, today, today, "day", 60_000);
  const answer = totals.data?.stations ?? [];
  const day = (id: number) => answer.find((s) => s.station_id === id)?.periods[0];
  const values = (pick: (id: number) => number | null | undefined, write: (value: number | null | undefined) => string) =>
    answer.map((s, i) => ({ station: s, color: seriesColor(i), value: pick(s.station_id), text: write(pick(s.station_id)) }));
  const count = (value: number | null | undefined) => (value == null ? "–" : number(value));
  const points = samples.data?.stations.some((s) => s.samples.length > 0) ?? false;
  return (
    <Card title={t("statistics.today")}>
      <div className="stack">
        {totals.isError && <ErrorView error={totals.error} />}
        {answer.length > 0 && (
          <div className="stats-tiles">
            <ComparedTile label={t("statistics.now")} values={values((id) => now.data?.stations.find((s) => s.station_id === id)?.total, count)} />
            <ComparedTile label={t("statistics.peak")} values={values((id) => day(id)?.peak, count)} />
            <ComparedTile label={t("statistics.listeningHours")} values={values((id) => day(id)?.listening_hours, listeningHours)} />
            <ComparedTile label={t("statistics.listeners")} values={values((id) => day(id)?.distinct_listeners, (v) => decimal(v, 0))} />
          </div>
        )}
        {samples.isError && <ErrorView error={samples.error} />}
        {samples.data &&
          (points ? (
            <LinesChart
              label={t("statistics.todayChart")}
              series={series(samples.data.stations, (s) => s.samples.map((p) => ({ at: p.at_unix_ms, value: p.listeners })))}
              from={from}
              to={to}
              step={60_000}
              ticks={timeTicks(from, to, 600)}
              tickLabel={shortTime}
              tooltip={(at, v) => (
                <>
                  {shortTime(at)}
                  {lineValues(v, (value) => number(value))}
                </>
              )}
            />
          ) : (
            <Empty>{t("statistics.noSamples")}</Empty>
          ))}
      </div>
    </Card>
  );
}

const FIGURES: {
  label: PlainKey;
  tip: PlainKey;
  pick: (a: Audience) => number | null | undefined;
  write: (value: number | null | undefined) => string;
}[] = [
  { label: "statistics.cume", tip: "statistics.cumeTip", pick: (a) => a.cume, write: (v) => decimal(v) },
  { label: "statistics.ttsl", tip: "statistics.ttslHint", pick: (a) => a.ttsl_hours, write: (v) => `${listeningHours(v)} h` },
  { label: "statistics.atsl", tip: "statistics.atslHint", pick: (a) => a.atsl_s, write: (v) => (v == null ? "–" : duration(v * 1000)) },
  { label: "statistics.aqh", tip: "statistics.aqhHint", pick: (a) => a.aqh, write: (v) => decimal(v) },
  { label: "statistics.peakAudience", tip: "statistics.peakTip", pick: (a) => a.peak, write: (v) => (v == null ? "–" : number(v)) },
  { label: "statistics.sessions", tip: "statistics.sessionsTip", pick: (a) => a.sessions, write: (v) => (v == null ? "–" : number(v)) },
];

/** A figure and its change against the previous period, in a table cell. */
function FigureCell({ current, previous, figure }: Readonly<{ current: Audience; previous: Audience; figure: (typeof FIGURES)[number] }>) {
  const ratio = change(figure.pick(current), figure.pick(previous));
  return (
    <td className="num">
      {figure.write(figure.pick(current))}
      <span className={`small ${ratio !== null && ratio < 0 ? "warn" : "muted"}`} style={{ display: "block" }} title={t("statistics.previousValue", { value: figure.write(figure.pick(previous)) })}>
        {changeText(ratio, locale())}
      </span>
    </td>
  );
}

export function CompareAudience({ view, period, zone }: Readonly<{ view: Compared; period: Period; zone: string | undefined }>) {
  const { from, to } = period;
  const stations = stationsParam(view);
  const figures = useQuery({
    queryKey: ["listeners", "stations", "audience", stations, from, to],
    queryFn: () => unwrap(api.GET("/api/v1/stations/listeners/audience", { params: { query: { stations, from, to } } })),
  });
  const data = figures.data;
  return (
    <Card title={t("statistics.audience")} actions={<Csv href={stationsCsvUrl("audience", view, { from, to })} />} flush={view.kind === "compare-all"}>
      {figures.isPending && <Loading />}
      {figures.isError && <ErrorView error={figures.error} />}
      {data && view.kind === "compare" && (
        <div className="stack">
          {legend(data.stations)}
          <div className="stats-tiles">
            {FIGURES.map((f) => (
              <ComparedTile
                key={f.label}
                label={t(f.label)}
                tip={t(f.tip)}
                values={data.stations.map((s, i) => ({ station: s, color: seriesColor(i), value: f.pick(s.current), text: f.write(f.pick(s.current)) }))}
              />
            ))}
          </div>
          {data.stations[0]?.current.peak_at_unix_ms != null && (
            <p className="muted small" style={{ margin: 0 }}>
              {data.stations.map((s) => `${s.name}: ${peakAt(s.current.peak_at_unix_ms, zone) ?? "–"}`).join(" · ")}
            </p>
          )}
        </div>
      )}
      {data && view.kind === "compare-all" && (
        <>
          <div className="table-wrap">
            <table className="table">
              <thead>
                <tr>
                  <th>{t("statistics.station")}</th>
                  {FIGURES.map((f) => (
                    <th key={f.label} className="num">
                      {t(f.label)}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {data.stations.map((s, i) => (
                  <tr key={s.station_id}>
                    <td style={{ whiteSpace: "nowrap" }}>
                      <span className="chart-swatch" style={{ background: seriesColor(i) }} aria-hidden="true" /> {s.name}
                    </td>
                    {FIGURES.map((f) => (
                      <FigureCell key={f.label} current={s.current} previous={s.previous} figure={f} />
                    ))}
                  </tr>
                ))}
                <tr>
                  <th scope="row" style={{ textAlign: "left" }}>
                    {t("statistics.allStations")}
                  </th>
                  {FIGURES.map((f) => (
                    <FigureCell key={f.label} current={data.total.current} previous={data.total.previous} figure={f} />
                  ))}
                </tr>
              </tbody>
            </table>
          </div>
          <p className="muted small" style={{ margin: "0.75rem 1rem" }}>
            {t("statistics.compareHelp")}
          </p>
        </>
      )}
    </Card>
  );
}

export function CompareOverTime({ view, period }: Readonly<{ view: Compared; period: Period }>) {
  const [grouping, setGrouping] = useState<Grouping>("day");
  const [measure, setMeasure] = useState<Measure>("listening_hours");
  const { from, to } = period;
  const totals = useStationsDays(view, from, to, grouping);
  const answer = totals.data?.stations ?? [];
  const days = [...new Set(answer.flatMap((s) => s.periods.map((p) => p.day)))].sort();
  const every = Math.max(1, Math.ceil(days.length / 8));
  const measureLabel = t(MEASURES.find((m) => m.value === measure)?.label ?? "statistics.listeningHours");
  return (
    <Card title={t("statistics.overTime")} actions={<Csv href={stationsCsvUrl("days", view, { from, to, group: grouping })} />}>
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
          <Segmented label={t("statistics.measure")} value={measure} onChange={setMeasure} options={MEASURES.map((m) => ({ value: m.value, label: t(m.label) }))} />
        </div>
        {totals.isPending && <Loading />}
        {totals.isError && <ErrorView error={totals.error} />}
        {totals.data && days.length === 0 && <Empty>{t("statistics.noData")}</Empty>}
        {days.length > 0 && (
          <LinesChart
            label={`${t("statistics.overTime")}: ${measureLabel}`}
            series={series(answer, (s) => s.periods.map((p) => ({ at: days.indexOf(p.day), value: p[measure] ?? 0 })))}
            from={0}
            to={Math.max(days.length - 1, 1)}
            step={1}
            ticks={days.map((_, i) => i).filter((i) => i % every === 0)}
            tickLabel={(i) => barLabel(days[i] ?? "", grouping)}
            tooltip={(i, values) => (
              <>
                {periodLabel(days[i] ?? "", grouping)}
                {lineValues(values, (value) => measureText({ ...EMPTY_PERIOD, [measure]: value }, measure))}
              </>
            )}
          />
        )}
      </div>
    </Card>
  );
}

const EMPTY_PERIOD: Schemas["PeriodTotals"] = {
  day: "",
  hour: null,
  days: 0,
  minutes: 0,
  peak: 0,
  average: 0,
  listening_hours: 0,
  sessions: null,
  average_session_s: null,
  distinct_listeners: null,
  listener_days: null,
};

type HourMeasure = "average" | "tune_in" | "tune_out";

export function CompareHours({ view, period }: Readonly<{ view: Compared; period: Period }>) {
  const [measure, setMeasure] = useState<HourMeasure>("average");
  const { from, to } = period;
  const stations = stationsParam(view);
  const byHour = useQuery({
    queryKey: ["listeners", "stations", "hours", stations, from, to],
    queryFn: () => unwrap(api.GET("/api/v1/stations/listeners/hours-of-day", { params: { query: { stations, from, to } } })),
  });
  const answer = byHour.data?.stations ?? [];
  const tunes = answer.some((s) => s.hours.some((h) => h.tune_in != null));
  const empty = byHour.data !== undefined && answer.every((s) => s.hours.length === 0);
  return (
    <Card title={t("statistics.hoursOfDay")} actions={<Csv href={stationsCsvUrl("hours-of-day", view, { from, to })} />}>
      {byHour.isPending && <Loading />}
      {byHour.isError && <ErrorView error={byHour.error} />}
      {empty && <Empty>{t("statistics.noData")}</Empty>}
      {byHour.data && !empty && (
        <div className="stack">
          {tunes && (
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
          )}
          <LinesChart
            label={t("statistics.hoursOfDayChart")}
            series={series(answer, (s) => s.hours.map((h) => ({ at: h.hour, value: (measure === "average" ? h.average : measure === "tune_in" ? h.tune_in : h.tune_out) ?? 0 })))}
            from={0}
            to={23}
            step={1}
            ticks={[0, 3, 6, 9, 12, 15, 18, 21]}
            tickLabel={hourLabel}
            tooltip={(hour, values) => (
              <>
                {hourLabel(hour)}
                {lineValues(values, (value) => decimal(value))}
              </>
            )}
          />
          <p className="muted small" style={{ margin: 0 }}>
            {measure === "average" ? t("statistics.hoursCompareHelp") : t("statistics.tuneHelp")}
          </p>
        </div>
      )}
    </Card>
  );
}

function useStationsBreakdown(by: By, view: StationsView, period: Period) {
  const { from, to } = period;
  const stations = stationsParam(view);
  return useQuery({
    queryKey: ["listeners", "stations", "breakdown", by, stations, from, to],
    queryFn: () => unwrap(api.GET("/api/v1/stations/listeners/breakdown", { params: { query: { stations, from, to, by } } })),
  });
}

/** A breakdown with a column of listener shares per station. */
function ShareColumns({ by, data, caption }: Readonly<{ by: By; data: Schemas["StationsBreakdown"]; caption: string }>) {
  const rows = shareColumns(
    data.total,
    data.stations.map((s) => s.groups),
  );
  return (
    <div className="table-wrap">
      <table className="table shares">
        <caption className="muted small" style={{ textAlign: "left" }}>
          {caption}
        </caption>
        <thead>
          <tr>
            <th>{caption}</th>
            {data.stations.map((s, i) => (
              <th key={s.station_id} className="num">
                <span className="chart-swatch" style={{ background: seriesColor(i) }} aria-hidden="true" /> {s.name}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map(({ share, shares }) => (
            <tr key={`${share.kind}-${share.name ?? ""}`}>
              <td>
                <span className={share.kind === "named" ? undefined : "muted"}>{shareName(share, by)}</span>
              </td>
              {shares.map((value, i) => (
                <td key={data.stations[i]?.station_id ?? i} className="num">
                  {percent(value, locale())}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export function CompareShares({ view, period }: Readonly<{ view: Compared; period: Period }>) {
  const countries = useStationsBreakdown("country", view, period);
  const devices = useStationsBreakdown("device", view, period);
  const players = useStationsBreakdown("player", view, period);
  const csv = (by: By) => stationsCsvUrl("breakdown", view, { from: period.from, to: period.to, by });
  const country = countries.data;
  const showCountries = !country || country.geoip != null || country.total.some((g) => g.kind === "named");
  const empty = devices.data?.total.length === 0 && players.data?.total.length === 0;
  return (
    <>
      {showCountries && (
        <Card title={t("statistics.countries")} actions={<Csv href={csv("country")} />}>
          {countries.isPending && <Loading />}
          {countries.isError && <ErrorView error={countries.error} />}
          {country &&
            (country.total.length === 0 ? (
              <Empty>{t("statistics.noShares")}</Empty>
            ) : (
              <div className="stack">
                <ShareColumns by="country" data={country} caption={t("statistics.country")} />
                <SharesHelp />
              </div>
            ))}
          {country?.geoip && <GeoipCredit database={country.geoip} />}
        </Card>
      )}
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
              <ShareColumns by="device" data={devices.data} caption={t("statistics.device")} />
              <ShareColumns by="player" data={players.data} caption={t("statistics.player")} />
            </div>
            <p className="muted small" style={{ margin: 0 }}>
              {t("statistics.sharesCompareHelp")}
            </p>
          </div>
        )}
      </Card>
    </>
  );
}
